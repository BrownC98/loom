use std::collections::HashMap;

use rusqlite::{Connection, params};

use api::graph::waits::{WaitKind, WaitTone};
use api::graph::{NodeState, NodeView, ProcessState, Status};
use api::ids::{AgentId, NodeId};

use super::waits::new_wait;
use super::{Graph, VIEW_COLUMNS, optional_json, to_json, view_from_row, watchdog};

const STALL_TEXT: &str = "기다릴 것이 없는데 쉽니다.";

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
    quiet: &dyn Fn(NodeId) -> bool,
) -> rusqlite::Result<Vec<NodeView>> {
    let since = since_of(db)?;
    let mut views = with_status(db, linked, tool, quiet)?;
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
    quiet: &dyn Fn(NodeId) -> bool,
) -> rusqlite::Result<Vec<NodeView>> {
    db.prepare(&query())?
        .query_map([], |row| {
            let view = view_from_row(row, linked)?;
            let derived = Derived {
                limited: row.get(20)?,
                idle: optional_json(row, 22)?,
                merged: optional_json(row, 23)?,
                rework: row.get(24)?,
            };
            Ok((view, derived))
        })?
        .map(|row| {
            let (mut view, derived) = row?;
            view.rework = derived.rework;
            view.status = status(&view, derived, tool, quiet);
            Ok(view)
        })
        .collect()
}

fn asks_human(view: &NodeView) -> impl Iterator<Item = i64> + '_ {
    view.waits
        .iter()
        .filter(|w| matches!(w.tone, WaitTone::Ask) && w.wait.kind != WaitKind::Stalled)
        .map(|w| w.wait.since)
}

fn status(
    view: &NodeView,
    derived: Derived,
    tool: &dyn Fn(NodeId) -> Option<String>,
    quiet: &dyn Fn(NodeId) -> bool,
) -> Status {
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
    derived
        .idle
        .filter(|idle| *idle != Status::Stalled || quiet(view.id))
        .unwrap_or(Status::Idle)
}

impl Graph {
    pub fn sync_stalls(&self, quiet: impl Fn(NodeId) -> bool) -> rusqlite::Result<()> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let stalled: Vec<NodeId> = with_status(&tx, self.linked, &|_| None, &quiet)?
            .into_iter()
            .filter(|v| v.status == Status::Stalled)
            .map(|v| v.id)
            .collect();
        let open: Vec<NodeId> = tx
            .prepare("SELECT node FROM stalls ORDER BY node")?
            .query_map([], |row| Ok(NodeId(row.get(0)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut changed = false;
        for node in open.iter().filter(|n| !stalled.contains(n)) {
            changed |= tx.execute("DELETE FROM stalls WHERE node = ?1", [node.0])? > 0;
        }
        for node in stalled.iter().filter(|n| !open.contains(n)) {
            let wait = new_wait(&tx, WaitKind::Stalled, STALL_TEXT.to_owned(), false, None)?;
            changed |= tx.execute(
                "INSERT INTO stalls (node, wait) VALUES (?1, ?2)",
                params![node.0, to_json(&wait)],
            )? > 0;
        }
        tx.commit()?;
        drop(db);
        if changed {
            self.publish_nodes();
        }
        Ok(())
    }
}
