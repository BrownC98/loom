//! 그래프: 노드 나무와 메일함. SQLite에 두고, 바뀔 때마다 화면에 이벤트 하나를 보낸다.
//! 부모-자식 규칙(깊이, 이름, 메시지 방향)은 나무와 메시지를 함께 보므로 이 모듈 한 곳에 둔다.
//! 근거: docs/design.md 「메시징」.

mod mailbox;
mod nodes;

use std::fmt;
use std::sync::{Mutex, MutexGuard};

use rusqlite::types::Type;
use rusqlite::{Connection, Row, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::futures::Notified;
use tokio::sync::{Notify, broadcast};
use ts_rs::TS;

pub use mailbox::{Delivery, Kind};

use crate::profile::{AgentId, BRIEF_LIMIT_BYTES};

/// 화면 하나가 밀려도 버티는 이벤트 수.
const EVENT_QUEUE: usize = 256;
/// 받는 노드마다 `queued` 메시지 상한. 근거: docs/design.md 「상한」.
const MAILBOX_LIMIT: u32 = 50;
/// 사람이 띄운 노드가 깊이 0이다. 근거: docs/design.md 「상한」.
const MAX_DEPTH: u32 = 2;
/// `loom done` 요약 상한. 근거: docs/design.md 「맥락과 크기」.
const SUMMARY_LIMIT_BYTES: usize = 1024;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NodeId(pub u32);

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum NodeState {
    Running,
    /// `summary`는 부모에게 보낸 요약이나 결과 파일 경로다.
    Done {
        summary: String,
    },
    /// `code`가 없으면 데몬이 다시 시작해 프로세스가 사라진 노드다.
    Exited {
        code: Option<u32>,
    },
}

#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct NodeView {
    pub id: NodeId,
    pub name: String,
    /// 카탈로그 에이전트. 직접 입력한 명령이면 없다.
    pub agent: Option<AgentId>,
    pub parent: Option<NodeId>,
    pub state: NodeState,
    /// 아직 전하지 않은(`queued`) 메시지 수.
    pub unread: u32,
}

#[derive(Clone, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum GraphEvent {
    Node(NodeView),
    /// `child`와 그 부모 사이 간선으로 메시지가 오갔다. 방향은 화면에 보이지 않는다.
    Message {
        child: NodeId,
    },
}

/// 규칙에 맞지 않아 거부한 요청. 문장은 그대로 에이전트나 사람에게 간다.
#[derive(Debug)]
pub enum Refusal {
    EmptyName,
    NameTaken(String),
    TooDeep,
    BriefTooLong(usize),
    SummaryTooLong(usize),
    MailboxFull(String),
    NotRelated(String),
    Ended(String),
    AlreadyDone,
    Launch(String),
    Db(rusqlite::Error),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::EmptyName => write!(f, "이름을 적는다."),
            Refusal::NameTaken(name) => write!(f, "{name}: 부모나 형제가 이미 이 이름을 쓴다."),
            Refusal::TooDeep => write!(
                f,
                "중첩 깊이 {MAX_DEPTH}를 넘는다. 자식을 더 만들지 않는다."
            ),
            Refusal::BriefTooLong(len) => write!(
                f,
                "본문이 {len}바이트다. {BRIEF_LIMIT_BYTES}바이트 이하로 줄이고, 긴 자료는 파일 경로를 적는다."
            ),
            Refusal::SummaryTooLong(len) => write!(
                f,
                "요약이 {len}바이트다. {SUMMARY_LIMIT_BYTES}바이트 이하로 줄이거나 `loom done --file <경로>`를 쓴다."
            ),
            Refusal::MailboxFull(name) => write!(
                f,
                "{name}의 메일함에 메시지가 {MAILBOX_LIMIT}개 쌓였다. 받는 노드가 `loom wait`로 읽은 뒤 다시 보낸다."
            ),
            Refusal::NotRelated(name) => {
                write!(
                    f,
                    "{name}: 내 부모나 자식이 아니다. 메시지는 부모와 자식 사이에만 보낸다."
                )
            }
            Refusal::Ended(name) => write!(f, "{name}: 노드 프로세스가 끝났다."),
            Refusal::AlreadyDone => write!(f, "이 노드는 이미 done이다."),
            Refusal::Launch(e) => write!(f, "노드를 띄우지 못했다: {e}"),
            Refusal::Db(e) => write!(f, "DB 오류: {e}"),
        }
    }
}

impl From<rusqlite::Error> for Refusal {
    fn from(e: rusqlite::Error) -> Self {
        Refusal::Db(e)
    }
}

pub struct Graph {
    db: Mutex<Connection>,
    events: broadcast::Sender<GraphEvent>,
    /// 메시지를 넣을 때마다 `wait` 모두를 깨운다. 깬 쪽이 자기 메시지인지 DB에서 본다.
    mail: Notify,
}

const VIEW_COLUMNS: &str = "SELECT n.id, n.name, n.agent, n.parent, n.state,
    (SELECT count(*) FROM messages m WHERE m.recipient = n.id AND m.delivered = 0)
    FROM nodes n";

impl Graph {
    /// 저장한 그래프를 연다. `running`이던 노드는 데몬과 함께 프로세스가 끝났으므로 코드 없는 `exited`로 바꾼다.
    pub fn open(conn: Connection) -> rusqlite::Result<Self> {
        conn.execute(
            "UPDATE nodes SET state = ?1 WHERE state ->> 'kind' = 'running'",
            [to_json(&NodeState::Exited { code: None })],
        )?;
        Ok(Self {
            db: Mutex::new(conn),
            events: broadcast::channel(EVENT_QUEUE).0,
            mail: Notify::new(),
        })
    }

    /// 지금 목록과 그 뒤의 이벤트를 빈틈없이 받는다.
    pub fn subscribe(&self) -> rusqlite::Result<(Vec<NodeView>, broadcast::Receiver<GraphEvent>)> {
        let db = self.lock();
        let views = db
            .prepare(&format!("{VIEW_COLUMNS} ORDER BY n.id"))?
            .query_map([], view_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok((views, self.events.subscribe()))
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.db.lock().unwrap()
    }

    /// 잠금을 쥔 채 부른다. 그래야 이벤트 순서가 DB 변경 순서와 같다.
    fn publish_node(&self, db: &Connection, id: NodeId) -> rusqlite::Result<()> {
        let view = db.query_row(
            &format!("{VIEW_COLUMNS} WHERE n.id = ?1"),
            [id.0],
            view_from_row,
        )?;
        // 보는 화면이 없으면 send가 실패한다. 화면은 붙을 때 목록 전체를 받으므로 버려도 된다.
        let _ = self.events.send(GraphEvent::Node(view));
        Ok(())
    }

    /// 다음 메시지가 들어올 때 끝나는 future. 메일함을 보기 전에 `enable()`해야 그 사이에 온 메시지를 놓치지 않는다.
    pub fn mail_changed(&self) -> Notified<'_> {
        self.mail.notified()
    }

    fn publish_message(&self, child: NodeId) {
        let _ = self.events.send(GraphEvent::Message { child });
        self.mail.notify_waiters();
    }
}

fn view_from_row(row: &Row) -> rusqlite::Result<NodeView> {
    Ok(NodeView {
        id: NodeId(row.get(0)?),
        name: row.get(1)?,
        agent: from_json(row, 2)?,
        parent: row.get::<_, Option<u32>>(3)?.map(NodeId),
        state: from_json(row, 4)?,
        unread: row.get(5)?,
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

fn set_state(db: &Connection, id: NodeId, state: &NodeState) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE nodes SET state = ?2 WHERE id = ?1",
        params![id.0, to_json(state)],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
