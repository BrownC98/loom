use rusqlite::{Connection, OptionalExtension, params};

use api::graph::events::{AgentEvent, AgentState, Entry, IdleEnd};

use super::events;
use super::turns::SUBMITTED_INPUT;
use super::{Graph, mailbox, optional_json, to_json};
use api::ids::NodeId;

#[derive(Clone, Debug, PartialEq)]
pub struct AgentChange {
    pub node: NodeId,
    pub from: Option<AgentState>,
    pub to: Option<AgentState>,
    pub by: AgentEvent,
}

fn load(db: &Connection, node: NodeId) -> rusqlite::Result<Option<Option<AgentState>>> {
    db.query_row(
        "SELECT s.state FROM nodes n LEFT JOIN agent_states s ON s.node = n.id WHERE n.id = ?1",
        [node.0],
        |row| optional_json(row, 0),
    )
    .optional()
}

fn store(db: &Connection, node: NodeId, state: &AgentState) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO agent_states (node, state) VALUES (?1, ?2)
         ON CONFLICT (node) DO UPDATE SET state = excluded.state",
        params![node.0, to_json(state)],
    )?;
    Ok(())
}

pub(super) fn start(db: &Connection, node: NodeId) -> rusqlite::Result<()> {
    store(db, node, &AgentState::Starting)
}

pub(super) fn apply(db: &Connection, node: NodeId, event: AgentEvent) -> rusqlite::Result<usize> {
    let Some(now) = load(db, node)? else {
        return Ok(0);
    };
    let moved = move_mail(db, node, &event)?;
    let to = match now.clone() {
        Some(state) => state.next(&event),
        None if event == AgentEvent::Relaunched => Some(AgentState::Starting),
        None => None,
    };
    let turn_began = to == Some(AgentState::Turn)
        && !matches!(
            &now,
            Some(AgentState::Turn | AgentState::Blocked { before: _ })
        );
    let replaced = matches!(
        event,
        AgentEvent::Relaunched | AgentEvent::Exited | AgentEvent::Slept
    );
    if replaced {
        db.execute(
            "UPDATE nodes SET drafted = 0, turn_flags = turn_flags & ~?2 WHERE id = ?1",
            params![node.0, SUBMITTED_INPUT],
        )?;
    } else if turn_began {
        db.execute(
            "UPDATE nodes SET drafted = 0 WHERE id = ?1 AND drafted = 1 AND (turn_flags & ?2) = 0",
            params![node.0, SUBMITTED_INPUT],
        )?;
    }
    if to == now {
        return Ok(moved);
    }
    match &to {
        Some(state) => store(db, node, state)?,
        None => drop(db.execute("DELETE FROM agent_states WHERE node = ?1", [node.0])?),
    }
    let entry = Entry::AgentStateChanged {
        node: events::who(db, node)?,
        from: now,
        to,
        by: event,
    };
    events::write(db, &entry, None)?;
    Ok(moved)
}

fn move_mail(db: &Connection, node: NodeId, event: &AgentEvent) -> rusqlite::Result<usize> {
    use AgentEvent as E;
    match event {
        E::TurnStarted | E::Busy | E::Idle => mailbox::read(db, node),
        E::TurnEnded | E::TurnEndedWaitOpened => {
            Ok(mailbox::read(db, node)? + mailbox::requeue_unprinted(db, node)?)
        }
        E::WaitOpened
        | E::WaitClosed {
            by: IdleEnd::Gone | IdleEnd::Error,
        } => mailbox::requeue_unprinted(db, node),
        E::Exited | E::RelaunchStarted | E::Slept => mailbox::requeue_unread(db, node),
        _ => Ok(0),
    }
}

pub(super) fn state_of(db: &Connection, node: NodeId) -> rusqlite::Result<Option<AgentState>> {
    Ok(load(db, node)?.flatten())
}

impl Graph {
    pub fn agent_event(
        &self,
        node: NodeId,
        event: AgentEvent,
    ) -> rusqlite::Result<Option<AgentState>> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let before = state_of(&tx, node)?;
        let moved = apply(&tx, node, event)?;
        tx.commit()?;
        if moved > 0 {
            self.publish_nodes();
            self.mail.notify_waiters();
        }
        Ok(before)
    }

    pub fn agent_state(&self, node: NodeId) -> rusqlite::Result<Option<AgentState>> {
        state_of(&self.lock(), node)
    }
}
