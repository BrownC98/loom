use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use api::graph::ProcessState;
use api::graph::events::{AgentEvent, Entry, IdleEnd};

use super::events;
use super::turns::{STATUS, SUBMITTED_INPUT};
use super::{Graph, mailbox, optional_json, process_of, process_state, to_json};
use api::ids::NodeId;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AgentState {
    Starting,
    Blocked { before: Box<AgentState> },
    Listening,
    Deaf,
    Turn,
    Mismatch,
}

impl AgentState {
    pub fn busy(&self) -> bool {
        !matches!(self, Self::Listening | Self::Deaf | Self::Blocked { .. })
    }

    pub fn cut(&self) -> bool {
        match self {
            Self::Listening => false,
            Self::Blocked { before } => before.cut(),
            Self::Starting | Self::Deaf | Self::Turn | Self::Mismatch => true,
        }
    }

    pub(super) fn shown(&self) -> ProcessState {
        match self {
            Self::Starting | Self::Turn | Self::Mismatch => ProcessState::Working,
            Self::Listening | Self::Deaf => ProcessState::Idle,
            Self::Blocked { before } => ProcessState::Blocked {
                before: Box::new(before.shown()),
            },
        }
    }

    fn name(state: Option<&Self>) -> &'static str {
        match state {
            Some(Self::Starting) => "starting",
            Some(Self::Blocked { .. }) => "blocked",
            Some(Self::Listening) => "listening",
            Some(Self::Deaf) => "deaf",
            Some(Self::Turn) => "turn",
            Some(Self::Mismatch) => "mismatch",
            None => "ended",
        }
    }

    pub fn next(self, event: &AgentEvent) -> Option<AgentState> {
        use AgentEvent as E;
        use AgentState as S;
        Some(match (self, event) {
            (_, E::Exited | E::Slept) => return None,
            (_, E::Relaunched) => S::Starting,
            (state, E::RelaunchStarted) => state,
            (_, E::VersionMismatch) => S::Mismatch,
            (S::Mismatch, _) => S::Mismatch,
            (S::Blocked { before }, E::DialogCleared) => *before,
            (S::Blocked { before }, E::Dialog) => S::Blocked { before },
            (S::Blocked { before }, event) => S::Blocked {
                before: Box::new(before.next(event)?),
            },
            (state, E::Dialog) => S::Blocked {
                before: Box::new(state),
            },
            (state, E::DialogCleared) => state,
            (_, E::TurnStarted | E::Busy | E::InputSent) => S::Turn,
            (_, E::WaitOpened | E::TurnEndedWaitOpened | E::Idle) => S::Listening,
            (_, E::Lost) => S::Deaf,
            (
                _,
                E::WaitClosed {
                    by: IdleEnd::Mail | IdleEnd::Turn { .. } | IdleEnd::Typed,
                },
            ) => S::Turn,
            (S::Listening, E::WaitClosed { .. }) => S::Deaf,
            (state, E::WaitClosed { .. }) => state,
            (S::Starting | S::Turn, E::TurnEnded) => S::Deaf,
            (state, E::TurnEnded) => state,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentChange {
    pub node: NodeId,
    pub to: Option<AgentState>,
    pub by: AgentEvent,
}

#[derive(PartialEq)]
pub(super) struct Was {
    process: ProcessState,
    agent: Option<AgentState>,
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

pub(super) fn was(db: &Connection, node: NodeId) -> rusqlite::Result<Option<Was>> {
    let Some(agent) = load(db, node)? else {
        return Ok(None);
    };
    let process = process_state(process_of(db, node)?, agent.as_ref());
    Ok(Some(Was { process, agent }))
}

pub(super) fn note(
    db: &Connection,
    node: NodeId,
    was: Option<Was>,
    by: AgentEvent,
) -> rusqlite::Result<()> {
    let (Some(was), Some(now)) = (was, self::was(db, node)?) else {
        return Ok(());
    };
    if was == now {
        return Ok(());
    }
    let entry = Entry::ProcessChanged {
        node: events::who(db, node)?,
        from: was.process,
        to: now.process,
        by,
        detail: AgentState::name(now.agent.as_ref()).to_owned(),
    };
    events::write(db, &entry, None)
}

pub(super) fn apply(db: &Connection, node: NodeId, event: AgentEvent) -> rusqlite::Result<usize> {
    let was = was(db, node)?;
    let moved = step(db, node, &event)?;
    note(db, node, was, event)?;
    Ok(moved)
}

pub(super) fn step(db: &Connection, node: NodeId, event: &AgentEvent) -> rusqlite::Result<usize> {
    let Some(now) = load(db, node)? else {
        return Ok(0);
    };
    let moved = move_mail(db, node, event)?;
    let to = match now.clone() {
        Some(state) => state.next(event),
        None if *event == AgentEvent::Relaunched => Some(AgentState::Starting),
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
        db.execute(
            "UPDATE nodes SET turn_flags = turn_flags & ~?2 WHERE id = ?1",
            params![node.0, STATUS],
        )?;
    }
    if to == now {
        return Ok(moved);
    }
    match &to {
        Some(state) => store(db, node, state)?,
        None => drop(db.execute("DELETE FROM agent_states WHERE node = ?1", [node.0])?),
    }
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
