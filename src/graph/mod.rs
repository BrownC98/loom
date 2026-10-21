//! 그래프: 노드 나무와 메일함. SQLite에 두고, 바뀔 때마다 화면에 이벤트 하나를 보낸다.
//! 바뀐 일은 이벤트 기록(`events`)에도 남긴다.
//! 나무 규칙(깊이, 이름, 받는 노드 찾기)은 나무와 메시지를 함께 보므로 이 모듈 한 곳에 둔다.
//! 근거: docs/design.md 「메시징」.

mod events;
mod mailbox;
mod nodes;
mod waits;

use std::collections::HashMap;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::sync::{Mutex, MutexGuard};

use rusqlite::types::Type;
use rusqlite::{Connection, Row, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::futures::Notified;
use tokio::sync::{Notify, broadcast};
use ts_rs::TS;

pub use events::{Entry, EventRecord, SwitchCause};
pub use mailbox::{Delivery, Kind, MessageId, REDELIVER_AFTER, Sent};
pub use nodes::{Active, Launched, Listed, NewChild, Relations};
pub use waits::{Attached, HUMAN, HumanWait, OpenQuestion, WaitDoc, WaitId, WaitKind, WaitSource};

use crate::profile::{AgentId, BRIEF_LIMIT_BYTES};
use crate::worktree::RepoId;

/// 화면 하나가 밀려도 버티는 이벤트 수.
const EVENT_QUEUE: usize = 256;
/// 받는 노드마다 `queued` 메시지 상한. 근거: docs/design.md 「상한」.
const MAILBOX_LIMIT: u32 = 50;
/// `loom done` 요약 상한. 근거: docs/design.md 「맥락과 크기」.
const SUMMARY_LIMIT_BYTES: usize = 1024;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NodeId(pub u32);

/// 노드 호스트가 주소 파일 이름에 쓴다.
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
    /// `summary`는 부모에게 보낸 요약이나 결과 파일 경로다.
    Done {
        summary: String,
    },
    /// `code`가 없으면 데몬이 다시 시작했을 때 프로세스가 사라졌던 노드다.
    Exited {
        code: Option<u32>,
    },
    /// `loom spawn --worktree`의 설정 스크립트가 이 코드로 끝나 에이전트를 띄우지 않았다.
    SetupFailed {
        code: u32,
    },
}

/// 노드가 도는 자리: 저장소와 그 워크트리(묶음). `worktree`는 `git worktree list`가 적는 절대 경로다.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Place {
    pub repo: RepoId,
    pub worktree: String,
}

#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct NodeView {
    pub id: NodeId,
    pub name: String,
    /// 카탈로그 에이전트. 명령(기본 셸 포함)이면 없다.
    pub agent: Option<AgentId>,
    pub parent: Option<NodeId>,
    pub state: NodeState,
    /// 아직 전하지 않은(`queued`) 메시지 수.
    pub unread: u32,
    /// 부모가 없어 보관한 메시지 수. 새 부모가 생기면 그 부모가 받는다.
    pub held: u32,
    /// CLI가 알린 모델 id 그대로다. 명령 노드이거나 CLI가 알리지 않았으면 없다.
    pub model: Option<String>,
    /// CLI가 알린 추론 강도 그대로다. 명령 노드이거나 CLI가 알리지 않았으면 없다.
    pub effort: Option<String>,
    pub place: Place,
    /// 노드가 지금 하는 일 한 줄. 근거: docs/design.md 「그래프와 화면」.
    /// `done` 요약, CLI 터미널 제목의 주제, 첫 지시문 순서로 처음 있는 것의 첫 줄이다. 모두 없으면 없다.
    pub topic: Option<String>,
    /// 사이드바 북마크 영역의 차례. 작을수록 먼저 북마크했다. 북마크하지 않았으면 없다.
    pub bookmark: Option<u32>,
    /// 사람의 답·승인을 기다린다. 근거: docs/design.md 「사람 응답 대기」.
    pub wait: Option<HumanWait>,
}

/// 프로필이 CLI에서 읽은 지금 모델과 추론 강도. 값은 CLI가 쓴 글 그대로다. 근거: docs/design.md 「에이전트 실행」.
#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct Model {
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[derive(Clone, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum GraphEvent {
    /// 연결할 때 먼저 한 번 보내는 노드 목록 전체. 화면은 가진 노드를 이 목록으로 바꾼다.
    /// 그래서 연결이 끊긴 동안 지운 노드가 화면에 남지 않는다(2026-10-20 사람 보고: 지운 노드 카드가 데몬을 다시 띄운 뒤에도 남음).
    Snapshot {
        nodes: Vec<NodeView>,
    },
    Node(NodeView),
    /// `child`와 그 부모 사이 간선으로 메시지가 `toward` 쪽으로 갔다.
    Message {
        child: NodeId,
        toward: Toward,
    },
    /// 부모·자식이 아닌 두 노드 사이로 메시지가 `from`에서 `to`로 갔다. 둘 사이에는 간선이 없다.
    Hop {
        from: NodeId,
        to: NodeId,
    },
    /// 노드를 지웠다. 자식은 부모 없이 남고 `Node` 이벤트로 따로 온다.
    Removed {
        id: NodeId,
    },
    /// 이벤트 기록에 한 줄을 더했다.
    Recorded {
        record: EventRecord,
    },
    /// 설정, 카드 자리, 카드 크기 가운데 하나가 바뀌었다. 값은 담지 않는다. 다른 화면은 세 가지를 다시 읽는다.
    ViewChanged,
}

/// 간선 위 메시지가 가는 쪽.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Toward {
    Parent,
    Child,
}

/// 노드 나무의 상한. 사람이 설정에서 바꾼다. 근거: docs/design.md 「상한」.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(default, rename_all = "camelCase")]
#[ts(export)]
pub struct TreeLimits {
    /// 사람이 띄운 노드가 깊이 0이다. 0이면 에이전트가 자식을 만들지 못한다. `None`이면 상한이 없다.
    pub max_depth: Option<u32>,
    /// 한 부모의 자식 수. `None`이면 상한이 없다.
    pub max_children: Option<u32>,
}

impl Default for TreeLimits {
    fn default() -> Self {
        Self {
            max_depth: Some(2),
            max_children: None,
        }
    }
}

/// 규칙에 맞지 않아 거부한 요청. 에이전트에게는 영어 문장(`Display`)이, 화면에는 코드와 값(JSON)이 간다.
/// 이벤트 기록에도 남기므로 읽을 수 있어야 한다(`Deserialize`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "code", content = "value", rename_all = "camelCase")]
#[ts(export)]
pub enum Refusal {
    EmptyName,
    NameTaken(String),
    /// `#`으로 시작하는 이름. `#번호`는 노드 번호 표기다.
    ReservedName(String),
    TooDeep,
    /// 값은 자식 수 상한이다.
    TooManyChildren(u32),
    BriefTooLong(#[ts(type = "number")] usize),
    SummaryTooLong(#[ts(type = "number")] usize),
    MailboxFull(String),
    /// 조정·중단의 대상이 부르는 노드의 부모나 자식이 아니다.
    NotRelated(String),
    /// `loom rm`의 대상이 부르는 노드의 자식이 아니다.
    NotChild(String),
    /// `loom rm`의 대상이 가진 자식을 부르는 노드가 데려갈 수 없다. 값은 대상 이름과 데려갈 수 없는 자식과 이유다.
    ChildrenStay(String),
    Ended(String),
    AlreadyDone,
    HeldFull,
    Cycle,
    NotFound(String),
    /// 이름이 맞는 노드가 둘 이상이다. `candidates`는 `#<번호> <이름> (<저장소>/<워크트리>)`를 쉼표로 이은 글이다.
    Ambiguous {
        name: String,
        candidates: String,
    },
    /// 조정·중단을 받지 못하는 노드다. 값은 보낸 쪽이 적은 이름이다.
    NotSteerable(String),
    /// 노드를 띄울 저장소가 목록에 없다(그 사이에 뺐다).
    NoRepo,
    /// `loom ask`의 질문이 비었다.
    EmptyQuestion,
    /// `loom ask`를 부모가 있는 노드가 불렀다. 사람에게는 사람이 띄운 노드만 묻는다.
    HasParent,
    /// 그 번호의 `loom ask` 질문이 열려 있지 않다(답했거나 지웠다). 문서가 없는 질문의 문서도 이것이다.
    NoWait(WaitId),
    /// 화면에서 보낸 답이 비었다.
    EmptyAnswer,
    Launch(String),
    Db(String),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::EmptyName => write!(f, "Give a name."),
            Refusal::NameTaken(name) => {
                write!(
                    f,
                    "{name}: a parent, sibling or child node already uses this name."
                )
            }
            Refusal::ReservedName(name) => write!(
                f,
                "{name}: names that start with # are for node IDs. Remove the #."
            ),
            Refusal::TooDeep => write!(
                f,
                "The tree depth limit is reached. Do not make more children."
            ),
            Refusal::TooManyChildren(max) => write!(
                f,
                "You already have {max} children. Remove a finished child or give the work to a child."
            ),
            Refusal::BriefTooLong(len) => write!(
                f,
                "The body is {len} bytes. Make it {BRIEF_LIMIT_BYTES} bytes or less, and put long material in a file and send its path."
            ),
            Refusal::SummaryTooLong(len) => write!(
                f,
                "The summary is {len} bytes. Make it {SUMMARY_LIMIT_BYTES} bytes or less, or use `loom done --file <path>`."
            ),
            Refusal::MailboxFull(name) => write!(
                f,
                "The inbox of {name} has {MAILBOX_LIMIT} messages. Send again after that node reads them with `loom wait`."
            ),
            Refusal::NotRelated(name) => write!(
                f,
                "{name}: not your parent or child. Steer and interrupt go only between a parent and its children. Use `loom send`."
            ),
            Refusal::NotChild(name) => write!(
                f,
                "{name}: not your child. You can remove only your own children."
            ),
            Refusal::ChildrenStay(list) => write!(
                f,
                "{list}. These children cannot move to you, so nothing changed. Remove or move them first."
            ),
            Refusal::Ended(name) => write!(f, "{name}: the node process has ended."),
            Refusal::AlreadyDone => write!(f, "This node is already done."),
            Refusal::HeldFull => write!(
                f,
                "You have no parent, and {MAILBOX_LIMIT} kept messages wait for one. Send again after a new parent adopts you."
            ),
            Refusal::Cycle => write!(
                f,
                "A node cannot have itself or its descendant as its parent."
            ),
            Refusal::NotFound(name) => write!(f, "{name}: no node has this name."),
            Refusal::NotSteerable(name) => write!(
                f,
                "{name}: this node cannot take steer or interrupt (only Claude, Codex and OpenCode 2 nodes can). Use `loom send`."
            ),
            Refusal::Ambiguous { name, candidates } => write!(
                f,
                "{name}: more than one node has this name: {candidates}. Use `#<id>`."
            ),
            Refusal::NoRepo => write!(
                f,
                "The repository is not in the list. Add the repository again."
            ),
            Refusal::EmptyQuestion => write!(f, "Give the question as one line."),
            Refusal::HasParent => write!(
                f,
                "You have a parent. Ask your parent with `loom send <parent> \"QUESTION: ...\"`. Only nodes that the human started ask the human."
            ),
            Refusal::NoWait(wait) => write!(f, "{wait}: this question is not open."),
            Refusal::EmptyAnswer => write!(f, "Write an answer."),
            Refusal::Launch(e) => write!(f, "Could not start the node: {e}"),
            Refusal::Db(e) => write!(f, "DB error: {e}"),
        }
    }
}

impl From<rusqlite::Error> for Refusal {
    fn from(e: rusqlite::Error) -> Self {
        Refusal::Db(e.to_string())
    }
}

pub struct Graph {
    db: Mutex<Store>,
    events: broadcast::Sender<GraphEvent>,
    /// 메시지를 넣을 때마다 `wait` 모두를 깨운다. 깬 쪽이 자기 메시지인지 DB에서 본다.
    mail: Notify,
    /// 실행 중인 노드 CLI의 터미널 제목에서 꺼낸 주제. PTY와 함께 사라지므로 DB에 두지 않는다.
    /// `db`를 잠근 채 잠근다. 반대 순서로 잠그지 않는다.
    titles: Mutex<HashMap<NodeId, String>>,
}

const VIEW_COLUMNS: &str = "SELECT n.id, n.name, n.agent, n.parent, n.state,
    (SELECT count(*) FROM messages m WHERE m.recipient = n.id AND m.delivered = 0),
    (SELECT count(*) FROM messages m WHERE m.recipient IS NULL AND m.sender = n.id),
    n.model, n.effort, n.repo, n.worktree,
    n.brief, n.bookmark, n.wait
    FROM nodes n";

struct Store {
    conn: Connection,
    /// 화면에 보낸 마지막 이벤트 기록 id.
    published: u32,
    /// 노드마다 기다리는 큐 메시지를 알린 마지막 id(`Graph::newly_queued`). 알림만 고르므로 DB에 두지 않는다.
    queue_noticed: HashMap<NodeId, MessageId>,
}

/// 잠근 DB. 놓을 때 아직 보내지 않은 이벤트 기록을 보낸다.
/// 기록은 바꾼 트랜잭션 안에서 쓴다. 되돌린 트랜잭션의 기록은 없으므로 보내지 않는다.
struct Db<'a> {
    store: MutexGuard<'a, Store>,
    events: &'a broadcast::Sender<GraphEvent>,
}

impl Deref for Db<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.store.conn
    }
}

impl DerefMut for Db<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.store.conn
    }
}

impl Drop for Db<'_> {
    fn drop(&mut self) {
        // 잠금을 쥔 채 보낸다. 그래야 기록 순서가 DB 순서와 같다.
        let Ok(new) = events::after(&self.store.conn, self.store.published) else {
            return;
        };
        for record in new {
            self.store.published = record.id;
            let _ = self.events.send(GraphEvent::Recorded { record });
        }
    }
}

impl Graph {
    /// 저장한 그래프를 연다. `running`이던 노드는 그대로 둔다. 프로세스가 살았는지는 데몬이 다시 붙어 보고 정한다(`active`, `lost`).
    pub fn open(conn: Connection) -> rusqlite::Result<Self> {
        let published = events::last_id(&conn)?;
        Ok(Self {
            db: Mutex::new(Store {
                conn,
                published,
                queue_noticed: HashMap::new(),
            }),
            events: broadcast::channel(EVENT_QUEUE).0,
            mail: Notify::new(),
            titles: Mutex::default(),
        })
    }

    /// 자리가 없는 옛 노드(DB 9단계 전)를 `home`으로 옮긴다. 데몬이 시작할 때 한 번 부른다.
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

    /// 그 워크트리 안의 노드 id. 워크트리를 지울 때 함께 지운다.
    pub fn nodes_in(&self, worktree: &str) -> rusqlite::Result<Vec<NodeId>> {
        self.lock()
            .prepare("SELECT id FROM nodes WHERE worktree = ?1 ORDER BY id")?
            .query_map([worktree], |row| Ok(NodeId(row.get(0)?)))?
            .collect()
    }

    /// 지금 목록과 그 뒤의 이벤트를 빈틈없이 받는다.
    pub fn subscribe(&self) -> rusqlite::Result<(Vec<NodeView>, broadcast::Receiver<GraphEvent>)> {
        let db = self.lock();
        let titles = self.titles.lock().unwrap();
        let views = db
            .prepare(&format!("{VIEW_COLUMNS} ORDER BY n.id"))?
            .query_map([], |row| view_from_row(row, &titles))?
            .collect::<rusqlite::Result<_>>()?;
        drop(titles);
        Ok((views, self.events.subscribe()))
    }

    fn lock(&self) -> Db<'_> {
        Db {
            store: self.db.lock().unwrap(),
            events: &self.events,
        }
    }

    /// 이벤트 콘솔을 열 때 읽는 최근 기록. 최근 것부터.
    pub fn recent_events(&self) -> rusqlite::Result<Vec<EventRecord>> {
        events::recent(&self.lock())
    }

    /// 보내기를 거절한 일을 남긴다. `reason`은 보낸 쪽이 받은 거절이다.
    pub fn record_refusal(
        &self,
        from: NodeId,
        to: &str,
        how: Kind,
        reason: &Refusal,
    ) -> rusqlite::Result<()> {
        let db = self.lock();
        let entry = events::Entry::Refused {
            how,
            from: events::who(&db, from)?,
            to: to.to_owned(),
            reason: reason.clone(),
        };
        events::write(&db, &entry, None)
    }

    /// 메시지·노드와 무관한 이벤트(계정 전환)를 남긴다.
    pub fn record(&self, entry: &Entry) -> rusqlite::Result<()> {
        events::write(&self.lock(), entry, None)
    }

    /// 잠금을 쥔 채 부른다. 그래야 이벤트 순서가 DB 변경 순서와 같다.
    fn publish_node(&self, db: &Connection, id: NodeId) -> rusqlite::Result<()> {
        let titles = self.titles.lock().unwrap();
        let view = db.query_row(&format!("{VIEW_COLUMNS} WHERE n.id = ?1"), [id.0], |row| {
            view_from_row(row, &titles)
        })?;
        drop(titles);
        // 보는 화면이 없으면 send가 실패한다. 화면은 붙을 때 목록 전체를 받으므로 버려도 된다.
        let _ = self.events.send(GraphEvent::Node(view));
        Ok(())
    }

    /// 다음 메시지가 들어올 때 끝나는 future. 메일함을 보기 전에 `enable()`해야 그 사이에 온 메시지를 놓치지 않는다.
    pub fn mail_changed(&self) -> Notified<'_> {
        self.mail.notified()
    }

    /// 화면 상태(설정·자리·크기)를 쓴 뒤 부른다. 보는 화면이 없으면 버린다.
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

fn view_from_row(row: &Row, titles: &HashMap<NodeId, String>) -> rusqlite::Result<NodeView> {
    let id = NodeId(row.get(0)?);
    let state = from_json(row, 4)?;
    let brief: Option<String> = row.get(11)?;
    let wait: Option<String> = row.get(13)?;
    Ok(NodeView {
        topic: topic(&state, titles.get(&id), brief.as_deref()),
        id,
        name: row.get(1)?,
        agent: from_json(row, 2)?,
        parent: row.get::<_, Option<u32>>(3)?.map(NodeId),
        state,
        unread: row.get(5)?,
        held: row.get(6)?,
        model: row.get(7)?,
        effort: row.get(8)?,
        place: Place {
            repo: RepoId(row.get(9)?),
            worktree: row.get(10)?,
        },
        bookmark: row.get(12)?,
        // 읽지 못하는 값(앞으로 바뀐 형식)은 대기가 없는 것으로 본다. 다음 신호가 다시 쓴다.
        wait: wait
            .and_then(|w| serde_json::from_str::<waits::Stored>(&w).ok())
            .map(|s| s.wait),
    })
}

/// 카드 주제: `done` 요약, 터미널 제목의 주제, 첫 지시문 순서로 처음 있는 것의 첫 줄.
fn topic(state: &NodeState, title: Option<&String>, brief: Option<&str>) -> Option<String> {
    let summary = match state {
        NodeState::Done { summary } => Some(summary.as_str()),
        _ => None,
    };
    [summary, title.map(String::as_str), brief]
        .into_iter()
        .flatten()
        .find_map(first_line)
        .map(str::to_owned)
}

/// 비지 않은 첫 줄(앞뒤 공백을 뺀다).
fn first_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

fn to_json(value: &impl Serialize) -> String {
    serde_json::to_string(value).expect("enum 값은 늘 직렬화된다")
}

fn from_json<T: DeserializeOwned>(row: &Row, idx: usize) -> rusqlite::Result<T> {
    let text: String = row.get(idx)?;
    serde_json::from_str(&text)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
}

fn state_of(db: &Connection, id: NodeId) -> rusqlite::Result<NodeState> {
    db.query_row("SELECT state FROM nodes WHERE id = ?1", [id.0], |row| {
        from_json(row, 0)
    })
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

/// 노드가 도는 자리를 사람이 읽는 글로: `<저장소 폴더>/<워크트리 폴더>`.
fn place_label(repo_path: &str, worktree: &str) -> String {
    let base = |path: &str| {
        path.rsplit(['/', '\\'])
            .find(|part| !part.is_empty())
            .unwrap_or(path)
            .to_owned()
    };
    format!("{}/{}", base(repo_path), base(worktree))
}

/// 이름이 `name`인 노드가 `ids` 여럿이라 거절한다. 다시 보낼 수 있게 후보를 번호와 자리로 적는다.
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

fn set_state(db: &Connection, id: NodeId, state: &NodeState) -> rusqlite::Result<()> {
    let entry = events::Entry::State {
        node: events::who(db, id)?,
        from: state_of(db, id)?,
        to: state.clone(),
    };
    db.execute(
        "UPDATE nodes SET state = ?2 WHERE id = ?1",
        params![id.0, to_json(state)],
    )?;
    events::write(db, &entry, None)
}

#[cfg(test)]
mod tests;
