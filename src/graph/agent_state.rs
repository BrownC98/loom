use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::events::{self, Entry, IdleEnd};
use super::{Graph, NodeId, optional_json, to_json};

/// 노드 에이전트가 지금 무엇을 하는지. 모든 에이전트 경로의 공통 사건(`AgentEvent`)이 `next` 하나로 바꾼다. 데몬 DB에 있어 데몬을 다시 띄워도 남는다.
/// 근거: R32 v2 ②A. 1단계는 그림자다. 옛 판정과 나란히 계산하고 다르면 `AgentStateDisagreed`를 남긴다. 결정은 아직 옛 판정이 한다.
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
    /// 데몬이 옛 프로세스를 끝내고 다시 띄우는 중이다.
    Relaunching,
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
    /// 훅이 턴 끝을 알렸다. 쉬는 대기는 따로 `WaitOpened`로 온다.
    TurnEnded,
    WaitOpened,
    /// 쉬는 대기가 끝났다. `Mail`이면 메일을 넘겨 턴이 시작한다.
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
    Relaunched,
    Slept,
    Exited,
    /// 노드 쪽 연결이 데몬이 받지 않는 판으로 요청했다.
    VersionMismatch,
}

/// 상태 기계와 견준 옛 판정.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ShadowCheck {
    /// `App::busy`(턴 중인가).
    Busy,
    /// `IdleWaits::waiting`(쉬는 대기가 열려 있는가).
    Listening,
}

impl AgentState {
    pub fn busy(&self) -> bool {
        !matches!(
            self,
            Self::Listening | Self::Deaf | Self::Asleep | Self::Ended
        )
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
            (_, E::RelaunchStarted) => S::Relaunching,
            (_, E::Relaunched) => S::Starting,
            (S::Relaunching, _) => S::Relaunching,
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
            (_, E::WaitOpened | E::ServerIdle) => S::Listening,
            (_, E::ServerLost) => S::Deaf,
            (
                _,
                E::WaitClosed {
                    by: IdleEnd::Mail | IdleEnd::Turn { .. },
                },
            ) => S::Turn,
            (
                S::Listening,
                E::WaitClosed {
                    by: IdleEnd::NewWait,
                },
            ) => S::Listening,
            (S::Listening, E::WaitClosed { .. }) => S::Deaf,
            (state, E::WaitClosed { .. }) => state,
            (S::Starting | S::Turn, E::TurnEnded) => S::Deaf,
            (state, E::TurnEnded) => state,
        }
    }

    fn agrees(&self, check: ShadowCheck, old: bool) -> bool {
        match check {
            ShadowCheck::Busy => self.busy() == old,
            ShadowCheck::Listening => (*self == Self::Listening) == old,
        }
    }
}

struct Stored {
    state: AgentState,
    seq: i64,
    noted: Option<i64>,
}

fn load(db: &Connection, node: NodeId) -> rusqlite::Result<Option<Stored>> {
    db.query_row(
        "SELECT s.state, coalesce(s.seq, 0), s.noted, n.process_exited, n.asleep
         FROM nodes n LEFT JOIN agent_states s ON s.node = n.id WHERE n.id = ?1",
        [node.0],
        |row| {
            let state = optional_json(row, 0)?.unwrap_or(match (row.get(3)?, row.get(4)?) {
                (_, true) => AgentState::Asleep,
                (true, false) => AgentState::Ended,
                (false, false) => AgentState::Starting,
            });
            Ok(Stored {
                state,
                seq: row.get(1)?,
                noted: row.get(2)?,
            })
        },
    )
    .optional()
}

fn store(db: &Connection, node: NodeId, stored: &Stored) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO agent_states (node, state, seq, noted, at) VALUES (?1, ?2, ?3, ?4, unixepoch())
         ON CONFLICT (node) DO UPDATE SET state = excluded.state, seq = excluded.seq,
             noted = excluded.noted, at = excluded.at",
        params![node.0, to_json(&stored.state), stored.seq, stored.noted],
    )?;
    Ok(())
}

pub(super) fn apply(db: &Connection, node: NodeId, event: AgentEvent) -> rusqlite::Result<()> {
    let Some(now) = load(db, node)? else {
        return Ok(());
    };
    let to = now.state.clone().next(&event);
    if to == now.state {
        return Ok(());
    }
    store(
        db,
        node,
        &Stored {
            state: to.clone(),
            seq: now.seq + 1,
            noted: now.noted,
        },
    )?;
    let entry = Entry::AgentStateChanged {
        node: events::who(db, node)?,
        from: now.state,
        to,
        by: event,
    };
    events::write(db, &entry, None)
}

fn compare(db: &Connection, node: NodeId, check: ShadowCheck, old: bool) -> rusqlite::Result<()> {
    let Some(now) = load(db, node)? else {
        return Ok(());
    };
    if now.state.agrees(check, old) || now.noted == Some(now.seq) {
        return Ok(());
    }
    let entry = Entry::AgentStateDisagreed {
        node: events::who(db, node)?,
        check,
        old,
        state: now.state.clone(),
    };
    store(
        db,
        node,
        &Stored {
            noted: Some(now.seq),
            ..now
        },
    )?;
    events::write(db, &entry, None)
}

pub(super) fn state_of(db: &Connection, node: NodeId) -> rusqlite::Result<Option<AgentState>> {
    Ok(load(db, node)?.map(|s| s.state))
}

impl Graph {
    pub fn agent_event(&self, node: NodeId, event: AgentEvent) -> rusqlite::Result<()> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        apply(&tx, node, event)?;
        tx.commit()
    }

    pub fn compare_busy(&self, node: NodeId, old: bool) -> rusqlite::Result<()> {
        self.compare(node, ShadowCheck::Busy, old)
    }

    pub fn compare_listening(&self, node: NodeId, old: bool) -> rusqlite::Result<()> {
        self.compare(node, ShadowCheck::Listening, old)
    }

    fn compare(&self, node: NodeId, check: ShadowCheck, old: bool) -> rusqlite::Result<()> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        compare(&tx, node, check, old)?;
        tx.commit()
    }

    #[cfg(test)]
    pub fn agent_state(&self, node: NodeId) -> rusqlite::Result<Option<AgentState>> {
        state_of(&self.lock(), node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATES: [&str; 9] = ["S", "bT", "L", "D", "T", "R", "M", "A", "E"];

    fn events() -> [AgentEvent; 21] {
        let closed = |by| AgentEvent::WaitClosed { by };
        [
            AgentEvent::TurnStarted,
            AgentEvent::TurnEnded,
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
            AgentEvent::Relaunched,
            AgentEvent::Slept,
            AgentEvent::Exited,
            AgentEvent::VersionMismatch,
        ]
    }

    const NEXT: [&str; 9] = [
        "T  D  L  T  S  T  S  S  S  S  S  bS S  T  L  D  R  S  A  E  M",
        "bT bD bL bT bT bT bT bT bT bT bT bT T  bT bL bD R  S  A  E  M",
        "T  L  L  T  L  T  D  D  D  D  D  bL L  T  L  D  R  S  A  E  M",
        "T  D  L  T  D  T  D  D  D  D  D  bD D  T  L  D  R  S  A  E  M",
        "T  D  L  T  T  T  T  T  T  T  T  bT T  T  L  D  R  S  A  E  M",
        "R  R  R  R  R  R  R  R  R  R  R  R  R  R  R  R  R  S  A  E  R",
        "M  M  M  M  M  M  M  M  M  M  M  M  M  M  M  M  R  S  A  E  M",
        "A  A  A  A  A  A  A  A  A  A  A  A  A  A  A  A  A  S  A  E  A",
        "E  E  E  E  E  E  E  E  E  E  E  E  E  E  E  E  E  S  E  E  E",
    ];

    fn state(code: &str) -> AgentState {
        match code {
            "S" => AgentState::Starting,
            "L" => AgentState::Listening,
            "D" => AgentState::Deaf,
            "T" => AgentState::Turn,
            "R" => AgentState::Relaunching,
            "M" => AgentState::Mismatch,
            "A" => AgentState::Asleep,
            "E" => AgentState::Ended,
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
    fn only_a_listening_deaf_asleep_or_ended_agent_is_not_busy() {
        let busy: Vec<&str> = STATES
            .into_iter()
            .filter(|code| state(code).busy())
            .collect();
        assert_eq!(busy, ["S", "bT", "T", "R", "M"]);
    }
}
