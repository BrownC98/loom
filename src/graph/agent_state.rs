use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::events::{self, Entry, IdleEnd};
use super::{Graph, NodeId, mailbox, optional_json, to_json};

/// 노드 에이전트가 지금 무엇을 하는지. 모든 에이전트 경로의 공통 사건(`AgentEvent`)이 `next` 하나로 바꾼다. 데몬 DB에 있어 데몬을 다시 띄워도 남는다.
/// 근거: R32 v3 ②A. 다시 띄우기·재우기·전달 판단이 이 상태를 읽는다.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum AgentState {
    /// 프로세스를 띄웠고 아직 첫 사건이 오지 않았다.
    Starting,
    /// 사람의 답을 기다리는 창이 에이전트를 막았다. `before`는 창이 닫히면 돌아갈 상태다.
    Blocked {
        before: Box<AgentState>,
    },
    /// 턴이 끝났고 메시지를 받을 길(쉬는 대기나 서버)이 열려 있다.
    Listening,
    /// 턴이 끝났는데 메시지를 받을 길이 없다.
    Deaf,
    Turn,
    /// 노드 쪽 연결의 판이 데몬과 맞지 않는다.
    Mismatch,
}

/// 에이전트 경로가 보내는 공통 사건.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum AgentEvent {
    /// 턴 안에서 에이전트가 메일함을 읽었다.
    TurnStarted,
    /// 옛 판의 훅이 턴 끝을 따로 알렸다. 쉬는 대기는 따로 `WaitOpened`로 온다. 훅이 닿지 않은 턴 끝의 증거(기록 파일과 화면)도 이 사건이다.
    TurnEnded,
    /// 훅이 한 요청으로 턴 끝을 알리고 쉬는 대기를 열었다.
    TurnEndedWaitOpened,
    WaitOpened,
    /// 노드의 마지막 쉬는 대기가 끝났다. `Mail`이면 메일을 넘겨 턴이 시작한다. 새 대기가 열린 뒤 끝난 옛 대기는 보내지 않는다.
    WaitClosed {
        by: IdleEnd,
    },
    /// 사람의 답을 기다리는 창이 떴다.
    Dialog,
    DialogCleared,
    /// 어댑터가 턴 중이라고 알렸다.
    #[serde(alias = "serverBusy")]
    Busy,
    /// 옛 기록만: 어댑터가 쉰다고 알렸다.
    #[serde(alias = "serverIdle")]
    Idle,
    /// 어댑터가 에이전트와 닿지 못한다고 알렸다.
    #[serde(alias = "serverLost")]
    Lost,
    /// 옛 기록만: 데몬이 턴을 여는 입력을 넣었고, 에이전트는 아직 상태를 알리지 않았다.
    InputSent,
    RelaunchStarted,
    Relaunched,
    Slept,
    Exited,
    /// 노드 쪽 연결이 데몬이 받지 않는 판으로 요청했다.
    VersionMismatch,
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
                    by: IdleEnd::Mail | IdleEnd::Turn { .. },
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
    if turn_began || replaced {
        db.execute(
            "UPDATE nodes SET drafted = 0 WHERE id = ?1 AND drafted = 1",
            [node.0],
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
