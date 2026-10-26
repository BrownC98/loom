mod agent_state;
mod events;
mod gate;
mod hosts;
mod idle;
mod mailbox;
mod nodes;
mod relaunch_plans;
mod turns;
mod waits;

use std::fmt;
use std::ops::{Deref, DerefMut};
use std::sync::{Mutex, MutexGuard};

use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::futures::Notified;
use tokio::sync::{Notify, broadcast, mpsc};
use ts_rs::TS;

pub use agent_state::{AgentChange, AgentEvent, AgentState};
pub use events::{
    AgentSwitch, BatchMember, BatchResult, Entry, EventLog, EventRecord, IdleEnd, SwitchCause,
    TurnCheck, Via, Who,
};
pub use gate::{BatchId, Ended, GateChoice, Merge, Queued, Reclaim, Round};
pub use idle::IdleId;
pub use mailbox::{Kept, PARENT, Report, Sent};
pub use nodes::{Active, Launched, Listed, NewChild, Relations};
pub use relaunch_plans::{Attempt, NewAgent, Phase, Plan, Planned};
pub use turns::TurnMark;
pub use waits::{HUMAN, HumanWait, WaitSource};

use crate::db::Database;
use crate::protocol::{AgentId, Kind, MessageKind, Refusal};
use crate::worktree::RepoId;

pub(crate) const EVENT_QUEUE: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NodeId(pub u32);

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum NodeState {
    Running,
    Done,
    /// `code`가 없으면 데몬이 다시 시작했을 때 프로세스가 사라졌던 노드다.
    Exited {
        code: Option<u32>,
    },
    /// `loom spawn --worktree`의 설정 스크립트가 이 코드로 끝나 에이전트를 띄우지 않았다.
    SetupFailed {
        code: u32,
    },
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Work {
    Running,
    Done,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ProcessOutcome {
    Asleep,
    Exited { code: u32 },
    Lost,
    HostGone,
    SetupFailed { code: u32 },
}

fn node_state(work: Work, process: Option<ProcessOutcome>) -> NodeState {
    match (work, process) {
        (Work::Done, _) => NodeState::Done,
        (Work::Running, None | Some(ProcessOutcome::Asleep | ProcessOutcome::HostGone)) => {
            NodeState::Running
        }
        (Work::Running, Some(ProcessOutcome::Exited { code })) => {
            NodeState::Exited { code: Some(code) }
        }
        (Work::Running, Some(ProcessOutcome::Lost)) => NodeState::Exited { code: None },
        (Work::Running, Some(ProcessOutcome::SetupFailed { code })) => {
            NodeState::SetupFailed { code }
        }
    }
}

/// 카드 보기. `Standard`는 카드에 터미널이 있다. `Simple`은 카드에 머리 정보만 있고, 고른 노드의 터미널은 오른쪽 칸에만 열린다.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CardView {
    Standard,
    Simple,
}

/// 노드가 도는 자리: 저장소와 그 워크트리(묶음). `worktree`는 `git worktree list`가 적는 절대 경로다.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Place {
    pub repo: RepoId,
    pub worktree: String,
}

#[derive(Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NodeView {
    pub id: NodeId,
    pub name: String,
    /// 카탈로그 에이전트. 명령(기본 셸 포함)이면 없다.
    pub agent: Option<AgentId>,
    pub parent: Option<NodeId>,
    pub state: NodeState,
    /// 아직 꺼내지 않은 메시지 수(모든 방식).
    pub unread: u32,
    /// CLI가 알린 모델 id 그대로다. 명령 노드이거나 CLI가 알리지 않았으면 없다.
    pub model: Option<String>,
    /// CLI가 알린 추론 강도 그대로다. 명령 노드이거나 CLI가 알리지 않았으면 없다.
    pub effort: Option<String>,
    pub place: Place,
    /// 사이드바 북마크 영역의 차례. 작을수록 먼저 북마크했다. 북마크하지 않았으면 없다.
    pub bookmark: Option<u32>,
    /// 사람이 「⋯」 메뉴로 이 노드에 고른 카드 보기. 없으면 설정의 기본 보기를 따른다.
    pub card_view: Option<CardView>,
    /// 마지막 턴을 끝낸 답 글. 심플 카드의 상태 줄이다(목업 docs/mockups/node-card-view.html ⑧). 답 글을 받은 적이 없으면 없다.
    pub reply: Option<String>,
    /// 사람의 답·승인을 기다린다. 열린 `loom ask` 질문들(물은 차례), 그 뒤에 신호의 대기 하나. 없으면 빈 목록이다.
    /// 근거: docs/design/메시징.md 「사람 응답 대기」.
    pub waits: Vec<HumanWait>,
    /// 노드 메뉴의 다시 띄우기. 없으면 메뉴에 보이지 않는다.
    pub relaunch: Option<RelaunchOffer>,
}

/// 노드 메뉴가 보이는 다시 띄우기. 데몬이 읽을 때 노드 상태, 에이전트의 메시지 연동, 다시 띄우기 계획에서 정한다.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RelaunchOffer {
    Ready,
    /// 계획이 있다. 턴 끝을 기다리거나 다시 띄우는 중이다.
    Queued,
}

#[derive(Clone, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum GraphEvent {
    /// 노드나 그 대기가 바뀌었다. 값은 담지 않는다. 화면은 `GET /api/nodes`로 노드 목록을 다시 읽는다. 연결하면 먼저 한 번 보낸다.
    Nodes,
    /// `child`와 그 부모 사이 간선으로 메시지가 `toward` 쪽으로 갔다.
    Message { child: NodeId, toward: Toward },
    /// 부모·자식이 아닌 두 노드 사이로 메시지가 `from`에서 `to`로 갔다. 둘 사이에는 간선이 없다.
    Hop { from: NodeId, to: NodeId },
    /// 이벤트 기록에 한 줄을 더했다.
    Recorded { record: Box<EventRecord> },
    /// 설정, 카드 자리, 카드 크기, 업데이트 상태 가운데 하나가 바뀌었거나 짝짓기 코드가 닫혔거나(짝지음, 다섯 번 틀림) 로그인 창의 명령이 끝났다. 값은 담지 않는다. 화면은 보고 있는 것을 다시 읽는다.
    ViewChanged,
    /// 데몬이 연결이 살아 있음을 알린다. 첫 `Nodes` 바로 뒤, `Pace`를 받은 바로 뒤, 다른 이벤트를 보내지 않은 동안 화면이 정한 간격(`Pace`)마다 보낸다.
    /// 화면은 `quietMs` 동안 아무 프레임도 받지 못하면 연결이 죽었다고 보고 그 연결만 다시 연다.
    Beat {
        #[serde(rename = "quietMs")]
        quiet_ms: u32,
    },
}

/// 간선 위 메시지가 가는 쪽.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Toward {
    Parent,
    Child,
}

pub struct Graph {
    db: Database,
    published: Mutex<u32>,
    events: broadcast::Sender<GraphEvent>,
    agent_tx: mpsc::UnboundedSender<AgentChange>,
    agent_rx: Mutex<Option<mpsc::UnboundedReceiver<AgentChange>>>,
    mail: Notify,
    linked: fn(&AgentId) -> bool,
}

pub(super) const REACHABLE: &str =
    "(process IS NULL OR process ->> 'kind' IN ('asleep', 'hostGone'))";

const VIEW_COLUMNS: &str = "SELECT n.id, n.name, n.agent, n.parent, n.state,
    (SELECT count(*) FROM messages m WHERE m.recipient = n.id AND m.delivered = 0 AND m.sent_at IS NULL),
    n.model, n.effort, n.repo, n.worktree,
    n.bookmark, n.wait,
    (SELECT json_group_array(json(d.wait)) FROM dones d WHERE d.node = n.id),
    (SELECT json_group_array(json(q.wait)) FROM questions q WHERE q.node = n.id),
    n.card_view, n.reply, n.process,
    EXISTS(SELECT 1 FROM relaunch_plans p WHERE p.node = n.id)
    FROM nodes n";

struct Db<'a> {
    conn: MutexGuard<'a, Connection>,
    published: MutexGuard<'a, u32>,
    events: &'a broadcast::Sender<GraphEvent>,
    agent: &'a mpsc::UnboundedSender<AgentChange>,
}

impl Deref for Db<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.conn
    }
}

impl DerefMut for Db<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

impl Drop for Db<'_> {
    fn drop(&mut self) {
        let Ok(new) = events::after(&self.conn, *self.published) else {
            return;
        };
        for record in new {
            *self.published = record.id;
            if let Entry::AgentStateChanged { node, from, to, by } = &record.entry
                && let Some(node) = node.id
            {
                let _ = self.agent.send(AgentChange {
                    node,
                    from: from.clone(),
                    to: to.clone(),
                    by: by.clone(),
                });
            }
            let _ = self.events.send(GraphEvent::Recorded {
                record: Box::new(record),
            });
        }
    }
}

impl Graph {
    pub fn open(db: Database, linked: fn(&AgentId) -> bool) -> rusqlite::Result<Self> {
        let published = events::last_id(&db.lock())?;
        relaunch_plans::settle_at_start(&db.lock())?;
        mailbox::requeue_handed(&db.lock())?;
        idle::forget_old_daemon(&db.lock())?;
        let (agent_tx, agent_rx) = mpsc::unbounded_channel();
        Ok(Self {
            db,
            published: Mutex::new(published),
            events: broadcast::channel(EVENT_QUEUE).0,
            agent_tx,
            agent_rx: Mutex::new(Some(agent_rx)),
            mail: Notify::new(),
            linked,
        })
    }

    pub fn settle(&self, home: &Place) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE nodes SET repo = ?1, worktree = ?2 WHERE repo IS NULL",
            params![home.repo.0, home.worktree],
        )?;
        Ok(())
    }

    pub fn place_of(&self, id: NodeId) -> rusqlite::Result<Place> {
        place_of(&self.lock(), id)
    }

    pub fn nodes_in(&self, worktree: &str) -> rusqlite::Result<Vec<NodeId>> {
        self.lock()
            .prepare("SELECT id FROM nodes WHERE worktree = ?1 ORDER BY id")?
            .query_map([worktree], |row| Ok(NodeId(row.get(0)?)))?
            .collect()
    }

    pub fn views(&self) -> rusqlite::Result<Vec<NodeView>> {
        self.lock()
            .prepare(&format!("{VIEW_COLUMNS} ORDER BY n.id"))?
            .query_map([], |row| view_from_row(row, self.linked))?
            .collect()
    }

    fn lock(&self) -> Db<'_> {
        Db {
            conn: self.db.lock(),
            published: self.published.lock().unwrap(),
            events: &self.events,
            agent: &self.agent_tx,
        }
    }

    pub fn agent_changes(&self) -> Option<mpsc::UnboundedReceiver<AgentChange>> {
        self.agent_rx.lock().unwrap().take()
    }

    pub fn event_log(&self) -> rusqlite::Result<EventLog> {
        events::log(&self.lock())
    }

    pub fn record_refusal(
        &self,
        from: NodeId,
        to: &str,
        how: Kind,
        what: Option<MessageKind>,
        reason: &Refusal,
        via: Option<&Via>,
    ) -> rusqlite::Result<()> {
        let db = self.lock();
        let entry = events::Entry::Refused {
            how,
            what,
            from: events::who(&db, from)?,
            to: to.to_owned(),
            reason: reason.clone(),
            via: via.cloned(),
            gate: None,
        };
        events::write(&db, &entry, None)
    }

    pub fn record_about(
        &self,
        node: NodeId,
        entry: impl FnOnce(Who) -> Entry,
    ) -> rusqlite::Result<()> {
        let db = self.lock();
        let entry = entry(events::who(&db, node)?);
        events::write(&db, &entry, None)
    }

    pub fn who(&self, node: NodeId) -> rusqlite::Result<Who> {
        events::who(&self.lock(), node)
    }

    pub fn record(&self, entry: &Entry) -> rusqlite::Result<()> {
        events::write(&self.lock(), entry, None)
    }

    fn publish_nodes(&self) {
        let _ = self.events.send(GraphEvent::Nodes);
    }

    pub fn mail_changed(&self) -> Notified<'_> {
        self.mail.notified()
    }

    pub fn wake_mail(&self) {
        self.mail.notify_waiters();
    }

    pub fn changes(&self) -> broadcast::Receiver<GraphEvent> {
        self.events.subscribe()
    }

    pub fn publish_view_changed(&self) {
        let _ = self.events.send(GraphEvent::ViewChanged);
    }

    fn publish_message(&self, child: NodeId, toward: Toward) {
        let _ = self.events.send(GraphEvent::Message { child, toward });
        self.mail.notify_waiters();
    }

    fn publish_hop(&self, from: NodeId, to: NodeId) {
        let _ = self.events.send(GraphEvent::Hop { from, to });
        self.mail.notify_waiters();
    }
}

fn view_from_row(row: &Row, linked: fn(&AgentId) -> bool) -> rusqlite::Result<NodeView> {
    let id = NodeId(row.get(0)?);
    let state = node_state(from_json(row, 4)?, optional_json(row, 16)?);
    let agent: Option<AgentId> = from_json(row, 2)?;
    let offered =
        agent.as_ref().is_some_and(linked) && matches!(state, NodeState::Running | NodeState::Done);
    let relaunch = offered.then_some(match row.get(17)? {
        true => RelaunchOffer::Queued,
        false => RelaunchOffer::Ready,
    });
    let wait: Option<String> = row.get(11)?;
    let done: String = row.get(12)?;
    let asked: String = row.get(13)?;
    let card_view: Option<String> = row.get(14)?;
    let mut waits: Vec<HumanWait> = serde_json::from_str(&asked).unwrap_or_default();
    waits.sort_by_key(|w| w.id.0);
    waits.extend(wait.and_then(|w| {
        serde_json::from_str::<waits::Stored>(&w)
            .ok()
            .map(|s| s.wait)
    }));
    let mut done: Vec<HumanWait> = serde_json::from_str(&done).unwrap_or_default();
    done.sort_by_key(|w| w.id.0);
    waits.extend(done);
    let waits = waits.into_iter().map(HumanWait::for_screen).collect();
    Ok(NodeView {
        id,
        name: row.get(1)?,
        agent,
        parent: row.get::<_, Option<u32>>(3)?.map(NodeId),
        state,
        unread: row.get(5)?,
        model: row.get(6)?,
        effort: row.get(7)?,
        place: Place {
            repo: RepoId(row.get(8)?),
            worktree: row.get(9)?,
        },
        bookmark: row.get(10)?,
        card_view: card_view.and_then(|v| serde_json::from_str(&v).ok()),
        reply: row.get(15)?,
        waits,
        relaunch,
    })
}

fn to_json(value: &impl Serialize) -> String {
    serde_json::to_string(value).expect("enum 값은 늘 직렬화된다")
}

fn from_json<T: DeserializeOwned>(row: &Row, idx: usize) -> rusqlite::Result<T> {
    let text: String = row.get(idx)?;
    serde_json::from_str(&text)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
}

fn optional_json<T: DeserializeOwned>(row: &Row, idx: usize) -> rusqlite::Result<Option<T>> {
    let text: Option<String> = row.get(idx)?;
    text.map(|text| {
        serde_json::from_str(&text)
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
    })
    .transpose()
}

fn state_of(db: &Connection, id: NodeId) -> rusqlite::Result<NodeState> {
    db.query_row(
        "SELECT state, process FROM nodes WHERE id = ?1",
        [id.0],
        |row| Ok(node_state(from_json(row, 0)?, optional_json(row, 1)?)),
    )
}

fn process_of(db: &Connection, id: NodeId) -> rusqlite::Result<Option<ProcessOutcome>> {
    db.query_row("SELECT process FROM nodes WHERE id = ?1", [id.0], |row| {
        optional_json(row, 0)
    })
    .optional()
    .map(Option::flatten)
}

fn parent_of(db: &Connection, id: NodeId) -> rusqlite::Result<Option<NodeId>> {
    db.query_row("SELECT parent FROM nodes WHERE id = ?1", [id.0], |row| {
        Ok(row.get::<_, Option<u32>>(0)?.map(NodeId))
    })
}

fn place_of(db: &Connection, id: NodeId) -> rusqlite::Result<Place> {
    db.query_row(
        "SELECT repo, worktree FROM nodes WHERE id = ?1",
        [id.0],
        |row| {
            Ok(Place {
                repo: RepoId(row.get(0)?),
                worktree: row.get(1)?,
            })
        },
    )
}

fn place_label(repo_path: &str, worktree: &str) -> String {
    let base = |path: &str| {
        path.rsplit(['/', '\\'])
            .find(|part| !part.is_empty())
            .unwrap_or(path)
            .to_owned()
    };
    format!("{}/{}", base(repo_path), base(worktree))
}

fn ambiguous(db: &Connection, name: &str, ids: &[NodeId]) -> rusqlite::Result<Refusal> {
    let mut found = db.prepare(
        "SELECT n.name, coalesce(r.path, ''), coalesce(n.worktree, '') FROM nodes n
         LEFT JOIN repos r ON r.id = n.repo WHERE n.id = ?1",
    )?;
    let candidates = ids
        .iter()
        .map(|id| {
            found.query_row([id.0], |row| {
                let place = place_label(&row.get::<_, String>(1)?, &row.get::<_, String>(2)?);
                Ok(format!("#{id} {} ({place})", row.get::<_, String>(0)?))
            })
        })
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Refusal::Ambiguous {
        name: name.to_owned(),
        candidates: candidates.join(", "),
    })
}

fn name_of(db: &Connection, id: NodeId) -> rusqlite::Result<String> {
    db.query_row("SELECT name FROM nodes WHERE id = ?1", [id.0], |row| {
        row.get(0)
    })
}

fn set_work(db: &Connection, id: NodeId, work: &Work) -> rusqlite::Result<()> {
    let from = state_of(db, id)?;
    db.execute(
        "UPDATE nodes SET state = ?2 WHERE id = ?1",
        params![id.0, to_json(work)],
    )?;
    note_state(db, id, from)
}

fn note_state(db: &Connection, id: NodeId, from: NodeState) -> rusqlite::Result<()> {
    let to = state_of(db, id)?;
    if to == from {
        return Ok(());
    }
    let entry = events::Entry::State {
        node: events::who(db, id)?,
        from,
        to,
    };
    events::write(db, &entry, None)
}
