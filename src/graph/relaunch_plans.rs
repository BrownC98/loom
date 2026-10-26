use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::{Graph, NodeId, agent_state, from_json, to_json};
use crate::protocol::{AgentId, ModelChoice};

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    AtTurnEnd,
    Running,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Plan {
    Resume,
    Switch(NewAgent),
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct NewAgent {
    pub agent: AgentId,
    #[serde(flatten)]
    pub choice: ModelChoice,
}

pub struct Planned {
    pub ended: bool,
    pub busy: bool,
    pub pending: Option<(Phase, Plan)>,
}

fn pending(db: &Connection, node: NodeId) -> rusqlite::Result<Option<(Phase, Plan)>> {
    db.query_row(
        "SELECT phase, plan FROM relaunch_plans WHERE node = ?1",
        [node.0],
        |row| Ok((from_json(row, 0)?, from_json(row, 1)?)),
    )
    .optional()
}

impl Graph {
    pub fn relaunch_plan(&self, node: NodeId) -> rusqlite::Result<Option<(Phase, Plan)>> {
        pending(&self.lock(), node)
    }

    pub fn plan_relaunch<R>(
        &self,
        node: NodeId,
        decide: impl FnOnce(Planned) -> (Option<(Phase, Plan)>, R),
    ) -> rusqlite::Result<R> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let ended: bool = tx
            .query_row(
                "SELECT process IS NOT NULL FROM nodes WHERE id = ?1",
                [node.0],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(true);
        let busy = agent_state::state_of(&tx, node)?.is_some_and(|s| s.busy());
        let (next, result) = decide(Planned {
            ended,
            busy,
            pending: pending(&tx, node)?,
        });
        match next {
            Some((phase, plan)) => tx.execute(
                "INSERT INTO relaunch_plans (node, phase, plan) VALUES (?1, ?2, ?3)
                 ON CONFLICT (node) DO UPDATE SET phase = excluded.phase, plan = excluded.plan",
                params![node.0, to_json(&phase), to_json(&plan)],
            )?,
            None => tx.execute("DELETE FROM relaunch_plans WHERE node = ?1", [node.0])?,
        };
        tx.commit()?;
        Ok(result)
    }

    pub fn limit_reached(&self, node: NodeId) -> rusqlite::Result<()> {
        self.lock()
            .execute("UPDATE nodes SET limited = 1 WHERE id = ?1", [node.0])?;
        Ok(())
    }

    pub fn limit_cleared(&self, node: NodeId) -> rusqlite::Result<()> {
        self.lock()
            .execute("UPDATE nodes SET limited = 0 WHERE id = ?1", [node.0])?;
        Ok(())
    }

    pub fn limited(&self, node: NodeId) -> rusqlite::Result<bool> {
        self.lock()
            .query_row("SELECT limited FROM nodes WHERE id = ?1", [node.0], |row| {
                row.get(0)
            })
            .optional()
            .map(|limited| limited.unwrap_or(false))
    }

    pub fn unread_mail(&self, node: NodeId) -> rusqlite::Result<bool> {
        self.lock().query_row(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE recipient = ?1 AND delivered = 0)",
            [node.0],
            |row| row.get(0),
        )
    }
}

pub(super) fn settle_running(db: &Connection) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE relaunch_plans SET phase = ?1 WHERE phase = ?2",
        params![to_json(&Phase::AtTurnEnd), to_json(&Phase::Running)],
    )?;
    Ok(())
}
