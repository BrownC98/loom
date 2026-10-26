use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::events::IdleEnd;
use super::{AgentEvent, AgentState, Graph, NodeId, agent_state};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IdleId(i64);

fn open_id(db: &Connection, node: NodeId) -> rusqlite::Result<Option<IdleId>> {
    db.query_row(
        "SELECT id FROM idle_requests WHERE node = ?1",
        [node.0],
        |row| row.get(0).map(IdleId),
    )
    .optional()
}

fn remove(db: &Connection, node: NodeId) -> rusqlite::Result<Option<IdleId>> {
    let id = open_id(db, node)?;
    db.execute("DELETE FROM idle_requests WHERE node = ?1", [node.0])?;
    Ok(id)
}

pub(super) fn forget_old_daemon(db: &Connection) -> rusqlite::Result<()> {
    db.execute("UPDATE idle_requests SET old = 1", []).map(drop)
}

impl Graph {
    fn idle_change<T>(
        &self,
        node: NodeId,
        change: impl FnOnce(&Transaction) -> rusqlite::Result<(T, Option<AgentEvent>)>,
    ) -> rusqlite::Result<T> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let (value, event) = change(&tx)?;
        let moved = match event {
            Some(event) => agent_state::apply(&tx, node, event)?,
            None => 0,
        };
        tx.commit()?;
        if moved > 0 {
            self.publish_node(&db, node)?;
            self.mail.notify_waiters();
        }
        Ok(value)
    }

    pub fn open_idle(
        &self,
        node: NodeId,
        event: AgentEvent,
    ) -> rusqlite::Result<(IdleId, Option<IdleId>)> {
        self.idle_change(node, |tx| {
            let replaced = remove(tx, node)?;
            tx.execute(
                "INSERT INTO idle_requests (node) SELECT id FROM nodes WHERE id = ?1",
                [node.0],
            )?;
            let id = open_id(tx, node)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            Ok(((id, replaced), Some(event)))
        })
    }

    pub fn reopen_idle(&self, node: NodeId, id: IdleId) -> rusqlite::Result<bool> {
        self.idle_change(node, |tx| {
            let free = open_id(tx, node)?.is_none()
                && agent_state::state_of(tx, node)? != Some(AgentState::Turn);
            if !free {
                return Ok((false, None));
            }
            tx.execute(
                "INSERT INTO idle_requests (id, node) VALUES (?1, ?2)",
                params![id.0, node.0],
            )?;
            Ok((true, Some(AgentEvent::WaitOpened)))
        })
    }

    pub fn close_idle(&self, node: NodeId, id: IdleId, by: IdleEnd) -> rusqlite::Result<()> {
        self.idle_change(node, |tx| {
            let closed = tx.execute(
                "DELETE FROM idle_requests WHERE node = ?1 AND id = ?2",
                params![node.0, id.0],
            )? > 0;
            Ok(((), closed.then_some(AgentEvent::WaitClosed { by })))
        })
    }

    pub fn end_idle(&self, node: NodeId, by: IdleEnd) -> rusqlite::Result<Option<IdleId>> {
        self.idle_change(node, |tx| {
            let id = remove(tx, node)?;
            Ok((id, id.map(|_| AgentEvent::WaitClosed { by })))
        })
    }

    pub fn idle_turn(&self, node: NodeId) -> rusqlite::Result<Option<IdleId>> {
        self.idle_change(node, |tx| {
            Ok((remove(tx, node)?, Some(AgentEvent::TurnStarted)))
        })
    }

    pub fn end_old_idle(&self, node: NodeId) -> rusqlite::Result<()> {
        self.idle_change(node, |tx| {
            let closed = tx.execute(
                "DELETE FROM idle_requests WHERE node = ?1 AND old = 1",
                [node.0],
            )? > 0;
            Ok((
                (),
                closed.then_some(AgentEvent::WaitClosed { by: IdleEnd::Gone }),
            ))
        })
    }
}
