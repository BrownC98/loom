use std::collections::HashMap;

use rusqlite::Connection;

use api::graph::waits::WaitTone;
use api::graph::{NodeState, NodeView, ProcessState, Status};
use api::ids::{AgentId, NodeId};

use super::{VIEW_COLUMNS, optional_json, view_from_row, watchdog};

struct Derived {
    limited: bool,
    idle: Option<Status>,
    merged: Option<Status>,
    rework: u32,
}

const MERGED: &str = r#"CASE WHEN n.state ->> 'kind' = 'done' THEN
    (SELECT json_object('kind', 'merged', 'target', g.merged_into, 'sha', coalesce((SELECT m.what ->> 'sha'
        FROM messages m WHERE m.recipient = n.id AND m.what ->> 'kind' = 'merged' AND m.what ->> 'branch' = g.branch
        ORDER BY m.id DESC LIMIT 1), ''))
    FROM gates g WHERE g.owner = n.id AND g.state = '"merged"' AND g.merged_into IS NOT NULL
        AND g.opened = (SELECT max(o.opened) FROM gates o WHERE o.owner = n.id)) END"#;

const REWORK: &str = "(SELECT count(*) FROM gates g JOIN messages m ON m.recipient = g.owner
    AND m.what ->> 'kind' = 'verdict' AND m.what ->> 'judgment' = 'block' AND m.what ->> 'branch' = g.branch
    AND m.id > coalesce((SELECT max(x.id) FROM messages x WHERE x.recipient = g.owner
        AND x.what ->> 'kind' = 'merged' AND x.what ->> 'branch' = g.branch), 0)
    WHERE g.owner = n.id AND g.opened = (SELECT max(o.opened) FROM gates o WHERE o.owner = n.id))";

const SINCE: &str = "SELECT entry ->> '$.node.id',
    max(CASE WHEN entry ->> 'kind' = 'processChanged' AND entry -> '$.from' != entry -> '$.to' THEN at END),
    min(CASE WHEN entry ->> 'kind' = 'created' THEN at END)
    FROM events WHERE CASE WHEN json_valid(entry) THEN entry ->> 'kind' IN ('processChanged', 'created') END
    GROUP BY 1";

fn query() -> String {
    format!(
        "{VIEW_COLUMNS},
    CASE WHEN n.process IS NULL AND n.state ->> 'kind' = 'running' AND EXISTS (SELECT 1 FROM agent_states s
        WHERE s.node = n.id AND s.state ->> 'kind' IN ('listening', 'deaf')) THEN {} END,
    {MERGED}, {REWORK}
    FROM nodes n ORDER BY n.id",
        watchdog::idle_status()
    )
}

fn since_of(db: &Connection) -> rusqlite::Result<HashMap<u32, i64>> {
    db.prepare(SINCE)?
        .query_map([], |row| {
            let node: Option<u32> = row.get(0)?;
            let changed: Option<i64> = row.get(1)?;
            let created: Option<i64> = row.get(2)?;
            Ok(node.zip(changed.or(created)))
        })?
        .filter_map(Result::transpose)
        .collect()
}

pub(super) fn views(
    db: &Connection,
    linked: fn(&AgentId) -> bool,
    tool: &dyn Fn(NodeId) -> Option<String>,
) -> rusqlite::Result<Vec<NodeView>> {
    let since = since_of(db)?;
    let mut views = with_status(db, linked, tool)?;
    for view in &mut views {
        view.since = match view.status {
            Status::Human => asks_human(view).min(),
            _ => None,
        }
        .or_else(|| since.get(&view.id.0).copied())
        .unwrap_or(0);
    }
    Ok(views)
}

fn with_status(
    db: &Connection,
    linked: fn(&AgentId) -> bool,
    tool: &dyn Fn(NodeId) -> Option<String>,
) -> rusqlite::Result<Vec<NodeView>> {
    db.prepare(&query())?
        .query_map([], |row| {
            let view = view_from_row(row, linked)?;
            let derived = Derived {
                limited: row.get(20)?,
                idle: optional_json(row, 21)?,
                merged: optional_json(row, 22)?,
                rework: row.get(23)?,
            };
            Ok((view, derived))
        })?
        .map(|row| {
            let (mut view, derived) = row?;
            view.rework = derived.rework;
            view.status = status(&view, derived, tool);
            Ok(view)
        })
        .collect()
}

fn asks_human(view: &NodeView) -> impl Iterator<Item = i64> + '_ {
    view.waits
        .iter()
        .filter(|w| matches!(w.tone, WaitTone::Ask))
        .map(|w| w.wait.since)
}

fn status(view: &NodeView, derived: Derived, tool: &dyn Fn(NodeId) -> Option<String>) -> Status {
    let running = view.state == NodeState::Running;
    if asks_human(view).next().is_some() {
        return Status::Human;
    }
    match (&view.state, &view.process) {
        (NodeState::SetupFailed { code }, _) => return Status::SetupFailed { code: *code },
        (NodeState::Running, ProcessState::Stopped { code }) => {
            return Status::Stopped { code: *code };
        }
        _ => {}
    }
    if running && derived.limited {
        return Status::Limited;
    }
    if view.process == ProcessState::Working {
        return Status::Working {
            tool: tool(view.id),
        };
    }
    if view.state == NodeState::Done {
        return derived.merged.unwrap_or(Status::Done);
    }
    derived.idle.unwrap_or(Status::Idle)
}
