use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::{Graph, NodeId, agent_state, from_json, mailbox, to_json};
use crate::protocol::{AgentId, ModelChoice};

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    AtTurnEnd,
    Running,
    Stopping,
    Stopped,
}

impl Phase {
    pub fn started(self) -> bool {
        self != Phase::AtTurnEnd
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Attempt {
    Deaf,
    Wake,
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
        let before = pending(&tx, node)?;
        let had = before.is_some();
        let (next, result) = decide(Planned {
            ended,
            busy,
            pending: before,
        });
        let changed = had != next.is_some();
        match next {
            Some((phase, plan)) => put(&tx, node, phase, &plan)?,
            None => tx.execute("DELETE FROM relaunch_plans WHERE node = ?1", [node.0])?,
        };
        tx.commit()?;
        if changed {
            self.publish_nodes();
        }
        Ok(result)
    }

    pub fn plan_resume(&self, node: NodeId) -> rusqlite::Result<bool> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let added = resume_at_turn_end(&tx, node, self.linked)?;
        tx.commit()?;
        if added {
            self.publish_nodes();
        }
        Ok(added)
    }

    pub fn claim_stop(&self, node: NodeId) -> rusqlite::Result<Option<Phase>> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let phase = pending(&tx, node)?.map(|(phase, _)| phase);
        if phase == Some(Phase::Running) {
            moved(&tx, node, &[Phase::Running], Phase::Stopping)?;
        }
        tx.commit()?;
        Ok(phase)
    }

    pub fn stop_exited(&self, node: NodeId) -> rusqlite::Result<bool> {
        moved(&self.lock(), node, &[Phase::Stopping], Phase::Stopped)
    }

    pub fn stop_lost(&self, node: NodeId) -> rusqlite::Result<bool> {
        moved(
            &self.lock(),
            node,
            &[Phase::Stopping, Phase::Stopped],
            Phase::Stopped,
        )
    }

    pub fn stop_dropped(&self, node: NodeId) -> rusqlite::Result<bool> {
        moved(&self.lock(), node, &[Phase::Stopping], Phase::Running)
    }

    pub fn relaunch_stopping(&self, node: NodeId) -> rusqlite::Result<bool> {
        Ok(pending(&self.lock(), node)?
            .is_some_and(|(phase, _)| matches!(phase, Phase::Stopping | Phase::Stopped)))
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

fn put(db: &Connection, node: NodeId, phase: Phase, plan: &Plan) -> rusqlite::Result<usize> {
    db.execute(
        "INSERT INTO relaunch_plans (node, phase, plan) VALUES (?1, ?2, ?3)
         ON CONFLICT (node) DO UPDATE SET phase = excluded.phase, plan = excluded.plan",
        params![node.0, to_json(&phase), to_json(plan)],
    )
}

pub(super) fn resume_at_turn_end(
    db: &Connection,
    node: NodeId,
    linked: fn(&AgentId) -> bool,
) -> rusqlite::Result<bool> {
    let live_agent = db
        .query_row(
            "SELECT agent, process IS NULL FROM nodes WHERE id = ?1",
            [node.0],
            |row| {
                let agent: Option<AgentId> = from_json(row, 0)?;
                Ok(agent.as_ref().is_some_and(linked) && row.get::<_, bool>(1)?)
            },
        )
        .optional()?
        .unwrap_or(false);
    if !live_agent || pending(db, node)?.is_some() {
        return Ok(false);
    }
    put(db, node, Phase::AtTurnEnd, &Plan::Resume)?;
    Ok(true)
}

fn moved(db: &Connection, node: NodeId, from: &[Phase], to: Phase) -> rusqlite::Result<bool> {
    let from: Vec<String> = from.iter().map(to_json).collect();
    let changed = db.execute(
        "UPDATE relaunch_plans SET phase = ?2 WHERE node = ?1 AND phase IN (SELECT value FROM json_each(?3))",
        params![node.0, to_json(&to), to_json(&from)],
    )?;
    Ok(changed > 0)
}

fn cursor(db: &Connection, node: NodeId, cause: Attempt) -> rusqlite::Result<Option<Option<u32>>> {
    db.query_row(
        "SELECT mail FROM recovery_attempts WHERE node = ?1 AND cause = ?2",
        params![node.0, to_json(&cause)],
        |row| row.get(0),
    )
    .optional()
}

fn mail_since(db: &Connection, node: NodeId, seen: Option<u32>) -> rusqlite::Result<bool> {
    Ok(mailbox::newest(db, node)? > seen)
}

impl Graph {
    pub fn attempted(&self, node: NodeId, cause: Attempt) -> rusqlite::Result<()> {
        let db = self.lock();
        let mail = mailbox::newest(&db, node)?;
        db.execute(
            "INSERT INTO recovery_attempts (node, cause, mail) VALUES (?1, ?2, ?3)
             ON CONFLICT (node, cause) DO UPDATE SET mail = excluded.mail",
            params![node.0, to_json(&cause), mail],
        )?;
        Ok(())
    }

    pub fn attempt_cleared(&self, node: NodeId, cause: Attempt) -> rusqlite::Result<()> {
        self.lock().execute(
            "DELETE FROM recovery_attempts WHERE node = ?1 AND cause = ?2",
            params![node.0, to_json(&cause)],
        )?;
        Ok(())
    }

    pub fn attempts_forgotten(&self, cause: Attempt) -> rusqlite::Result<()> {
        forget_attempts(&self.lock(), cause)
    }

    pub fn attempt_held(&self, node: NodeId, cause: Attempt) -> rusqlite::Result<bool> {
        Ok(cursor(&self.lock(), node, cause)?.is_some())
    }

    pub fn mail_since_attempt(&self, node: NodeId, cause: Attempt) -> rusqlite::Result<bool> {
        let db = self.lock();
        match cursor(&db, node, cause)? {
            None => Ok(true),
            Some(seen) => mail_since(&db, node, seen),
        }
    }

    pub fn waking_mail(&self, node: NodeId) -> rusqlite::Result<bool> {
        Ok(mailbox::newest(&self.lock(), node)?.is_some())
    }

    pub fn mailed_since_attempt(&self, cause: Attempt) -> rusqlite::Result<Vec<NodeId>> {
        let db = self.lock();
        let held = db
            .prepare("SELECT node, mail FROM recovery_attempts WHERE cause = ?1 ORDER BY node")?
            .query_map([to_json(&cause)], |row| {
                Ok((NodeId(row.get(0)?), row.get::<_, Option<u32>>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut mailed = Vec::new();
        for (node, seen) in held {
            if mail_since(&db, node, seen)? {
                mailed.push(node);
            }
        }
        Ok(mailed)
    }
}

pub(super) fn settle_at_start(db: &Connection) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE relaunch_plans SET phase = ?1 WHERE phase = ?2",
        params![to_json(&Phase::AtTurnEnd), to_json(&Phase::Running)],
    )?;
    forget_attempts(db, Attempt::Wake)
}

fn forget_attempts(db: &Connection, cause: Attempt) -> rusqlite::Result<()> {
    db.execute(
        "DELETE FROM recovery_attempts WHERE cause = ?1",
        [to_json(&cause)],
    )?;
    Ok(())
}
