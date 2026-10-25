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
    /// 데몬이 옛 프로세스를 끝내고 다시 띄우는 중이다. `before`는 옛 프로세스가 끝나지 않아 다시 띄우기가 실패하면 돌아갈 상태다. 그동안 온 사건도 `before`에 적용한다.
    Relaunching {
        before: Box<AgentState>,
    },
    /// 노드 쪽 연결의 판이 데몬과 맞지 않는다.
    Mismatch,
    Asleep,
    Ended,
}

/// 에이전트 경로가 보내는 공통 사건.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum AgentEvent {
    /// 턴 안의 훅이 메일함을 읽었다.
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
    /// 서버(Codex, OpenCode 2)가 턴 중이라고 알렸다.
    ServerBusy,
    /// 서버가 쉰다고 알렸다. 데몬이 서버로 메시지를 넘기므로 듣는 중이다.
    ServerIdle,
    /// 서버 연결이 끊겼다.
    ServerLost,
    RelaunchStarted,
    /// 옛 프로세스가 끝나지 않아 다시 띄우기를 그만두었다. 옛 프로세스가 그대로 돈다.
    RelaunchFailed,
    Relaunched,
    Slept,
    Exited,
    /// 노드 쪽 연결이 데몬이 받지 않는 판으로 요청했다.
    VersionMismatch,
}

impl AgentState {
    pub fn busy(&self) -> bool {
        !matches!(
            self,
            Self::Listening | Self::Deaf | Self::Blocked { .. } | Self::Asleep | Self::Ended
        )
    }

    pub fn cut(&self) -> bool {
        match self {
            Self::Listening | Self::Asleep | Self::Ended => false,
            Self::Blocked { before } | Self::Relaunching { before } => before.cut(),
            Self::Starting | Self::Deaf | Self::Turn | Self::Mismatch => true,
        }
    }

    pub fn next(self, event: &AgentEvent) -> AgentState {
        use AgentEvent as E;
        use AgentState as S;
        match (self, event) {
            (_, E::Exited) => S::Ended,
            (S::Ended, E::Relaunched) => S::Starting,
            (S::Ended, _) => S::Ended,
            (_, E::Slept) => S::Asleep,
            (S::Asleep, E::Relaunched) => S::Starting,
            (S::Asleep, _) => S::Asleep,
            (_, E::Relaunched) => S::Starting,
            (S::Relaunching { before }, E::RelaunchFailed) => *before,
            (S::Relaunching { before }, E::RelaunchStarted) => S::Relaunching { before },
            (S::Relaunching { before }, event) => S::Relaunching {
                before: Box::new(before.next(event)),
            },
            (state, E::RelaunchStarted) => S::Relaunching {
                before: Box::new(state),
            },
            (state, E::RelaunchFailed) => state,
            (_, E::VersionMismatch) => S::Mismatch,
            (S::Mismatch, _) => S::Mismatch,
            (S::Blocked { before }, E::DialogCleared) => *before,
            (S::Blocked { before }, E::Dialog) => S::Blocked { before },
            (S::Blocked { before }, event) => S::Blocked {
                before: Box::new(before.next(event)),
            },
            (state, E::Dialog) => S::Blocked {
                before: Box::new(state),
            },
            (state, E::DialogCleared) => state,
            (_, E::TurnStarted | E::ServerBusy) => S::Turn,
            (_, E::WaitOpened | E::TurnEndedWaitOpened | E::ServerIdle) => S::Listening,
            (_, E::ServerLost) => S::Deaf,
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
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentChange {
    pub node: NodeId,
    pub from: AgentState,
    pub to: AgentState,
    pub by: AgentEvent,
}

struct Stored {
    state: AgentState,
    seq: i64,
}

fn load(db: &Connection, node: NodeId) -> rusqlite::Result<Option<Stored>> {
    db.query_row(
        "SELECT s.state, coalesce(s.seq, 0), n.process_exited, n.asleep
         FROM nodes n LEFT JOIN agent_states s ON s.node = n.id WHERE n.id = ?1",
        [node.0],
        |row| {
            let state = optional_json(row, 0)?.unwrap_or(match (row.get(2)?, row.get(3)?) {
                (_, true) => AgentState::Asleep,
                (true, false) => AgentState::Ended,
                (false, false) => AgentState::Starting,
            });
            Ok(Stored {
                state,
                seq: row.get(1)?,
            })
        },
    )
    .optional()
}

fn store(db: &Connection, node: NodeId, stored: &Stored) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO agent_states (node, state, seq, at) VALUES (?1, ?2, ?3, unixepoch())
         ON CONFLICT (node) DO UPDATE SET state = excluded.state, seq = excluded.seq, at = excluded.at",
        params![node.0, to_json(&stored.state), stored.seq],
    )?;
    Ok(())
}

pub(super) fn apply(db: &Connection, node: NodeId, event: AgentEvent) -> rusqlite::Result<usize> {
    let Some(now) = load(db, node)? else {
        return Ok(0);
    };
    let moved = move_mail(db, node, &event)?;
    let to = now.state.clone().next(&event);
    if to == now.state {
        return Ok(moved);
    }
    store(
        db,
        node,
        &Stored {
            state: to.clone(),
            seq: now.seq + 1,
        },
    )?;
    let entry = Entry::AgentStateChanged {
        node: events::who(db, node)?,
        from: now.state,
        to,
        by: event,
    };
    events::write(db, &entry, None)?;
    Ok(moved)
}

fn move_mail(db: &Connection, node: NodeId, event: &AgentEvent) -> rusqlite::Result<usize> {
    use AgentEvent as E;
    match event {
        E::TurnStarted | E::ServerBusy | E::ServerIdle => mailbox::read(db, node),
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
    Ok(load(db, node)?.map(|s| s.state))
}

impl Graph {
    pub fn agent_event(&self, node: NodeId, event: AgentEvent) -> rusqlite::Result<()> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let moved = apply(&tx, node, event)?;
        tx.commit()?;
        if moved > 0 {
            self.publish_node(&db, node)?;
            self.mail.notify_waiters();
        }
        Ok(())
    }

    pub fn agent_state(&self, node: NodeId) -> rusqlite::Result<Option<AgentState>> {
        state_of(&self.lock(), node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATES: [&str; 9] = ["S", "bT", "L", "D", "T", "rT", "M", "A", "E"];

    fn events() -> [AgentEvent; 23] {
        let closed = |by| AgentEvent::WaitClosed { by };
        [
            AgentEvent::TurnStarted,
            AgentEvent::TurnEnded,
            AgentEvent::TurnEndedWaitOpened,
            AgentEvent::WaitOpened,
            closed(IdleEnd::Mail),
            closed(IdleEnd::NewWait),
            closed(IdleEnd::Turn { hook: "h".into() }),
            closed(IdleEnd::Relaunch),
            closed(IdleEnd::Exit),
            closed(IdleEnd::Sleep),
            closed(IdleEnd::Gone),
            closed(IdleEnd::Error),
            AgentEvent::Dialog,
            AgentEvent::DialogCleared,
            AgentEvent::ServerBusy,
            AgentEvent::ServerIdle,
            AgentEvent::ServerLost,
            AgentEvent::RelaunchStarted,
            AgentEvent::RelaunchFailed,
            AgentEvent::Relaunched,
            AgentEvent::Slept,
            AgentEvent::Exited,
            AgentEvent::VersionMismatch,
        ]
    }

    const NEXT: [&str; 9] = [
        "T  D  L  L  T  S  T  S  S  S  S  S  bS  S  T  L  D  rS  S  S  A  E  M",
        "bT  bD  bL  bL  bT  bT  bT  bT  bT  bT  bT  bT  bT  T  bT  bL  bD  rbT  bT  S  A  E  M",
        "T  L  L  L  T  D  T  D  D  D  D  D  bL  L  T  L  D  rL  L  S  A  E  M",
        "T  D  L  L  T  D  T  D  D  D  D  D  bD  D  T  L  D  rD  D  S  A  E  M",
        "T  D  L  L  T  T  T  T  T  T  T  T  bT  T  T  L  D  rT  T  S  A  E  M",
        "rT  rD  rL  rL  rT  rT  rT  rT  rT  rT  rT  rT  rbT  rT  rT  rL  rD  rT  T  S  A  E  rM",
        "M  M  M  M  M  M  M  M  M  M  M  M  M  M  M  M  M  rM  M  S  A  E  M",
        "A  A  A  A  A  A  A  A  A  A  A  A  A  A  A  A  A  A  A  S  A  E  A",
        "E  E  E  E  E  E  E  E  E  E  E  E  E  E  E  E  E  E  E  S  E  E  E",
    ];

    fn state(code: &str) -> AgentState {
        match code {
            "S" => AgentState::Starting,
            "L" => AgentState::Listening,
            "D" => AgentState::Deaf,
            "T" => AgentState::Turn,
            "M" => AgentState::Mismatch,
            "A" => AgentState::Asleep,
            "E" => AgentState::Ended,
            relaunching if relaunching.starts_with('r') => AgentState::Relaunching {
                before: Box::new(state(&relaunching[1..])),
            },
            blocked => AgentState::Blocked {
                before: Box::new(state(blocked.strip_prefix('b').unwrap())),
            },
        }
    }

    #[test]
    fn every_state_and_event_pair_has_its_next_state() {
        for (from, row) in STATES.iter().zip(NEXT) {
            let row: Vec<&str> = row.split_whitespace().collect();
            assert_eq!(row.len(), events().len(), "row {from}");
            for (event, to) in events().iter().zip(row) {
                assert_eq!(
                    state(from).next(event),
                    state(to),
                    "{from} + {event:?} goes to {to}"
                );
            }
        }
    }

    #[test]
    fn only_a_listening_deaf_blocked_asleep_or_ended_agent_is_not_busy() {
        let busy: Vec<&str> = STATES
            .into_iter()
            .filter(|code| state(code).busy())
            .collect();
        assert_eq!(busy, ["S", "T", "rT", "M"]);
    }
}
