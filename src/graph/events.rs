//! 이벤트 기록: 노드와 메시지에 일어난 일을 바꾼 트랜잭션 안에서 `events` 테이블에 한 줄씩 남긴다.
//! 화면의 이벤트 콘솔이 읽는다. 잠금을 놓을 때 새 줄을 웹소켓으로 보낸다(`Db`의 `Drop`).
//! 근거: docs/mockups/graph-panels.html 결정 표 「이벤트 저장」·「이벤트 화면 전달」.

use rusqlite::{Connection, Row, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{Kind, MessageId, NodeId, NodeState, Refusal, WaitDoc, WaitId, from_json, to_json};
use crate::accounts::AccountId;
use crate::choices::{Choice, choices};
use crate::link_text::{TextPart, parts, path_link};
use crate::profile::{AgentId, Provider};
use crate::protocol::{Field, Finding, GateState, GateVerb, MessageKind};

/// 남기는 이벤트 수 상한. 노드 50개가 각각 메시지 200개를 주고받는 하루치다.
const KEEP: u32 = 10_000;
/// 이벤트 콘솔을 열 때 읽는 수. 한 화면 스크롤 몇 번 분량이다.
const RECENT: u32 = 500;

/// 이벤트 하나. 노드 이름을 함께 남긴다. 노드를 지워도 이벤트를 읽을 수 있어야 한다.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Who {
    /// 메시지를 보낸 노드를 지운 뒤 받음을 확인하면 보낸 노드의 id를 모른다. 그때는 없다.
    pub id: Option<NodeId>,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Step {
    /// 받는 노드 메일함에 넣었다.
    Sent,
    /// 부모가 없어 보관했다. 새 부모가 생기면 그 부모가 받는다.
    Held,
    /// 받는 노드가 받음을 확인했다.
    Delivered,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Entry {
    Created {
        node: Who,
        parent: Option<Who>,
        agent: Option<AgentId>,
    },
    State {
        node: Who,
        from: NodeState,
        to: NodeState,
    },
    /// 부모가 바뀌었다. `from`이 없으면 부모 없던 노드다.
    Adopted {
        node: Who,
        from: Option<Who>,
        to: Who,
        /// 부모가 없던 동안 보관해 새 부모에게 옮긴 메시지 수. 이 수를 남기기 전 기록은 0이다.
        #[serde(default)]
        held: u32,
    },
    Removed {
        node: Who,
    },
    /// 사람이 이름을 바꿨다. `node`는 새 이름, `from`은 옛 이름이다.
    Renamed {
        node: Who,
        from: String,
    },
    /// `to`가 없으면 보관한 메시지다.
    Message {
        how: Kind,
        /// 메시지 종류와 칸. 종류를 남기기 전의 옛 기록에는 없다. Entry의 serde 태그가 `kind`라 이름이 `what`이다.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        what: Option<MessageKind>,
        from: Who,
        to: Option<Who>,
        step: Step,
        /// 보낸 입구. 받음 확인과 데몬이 만든 메시지, 옛 기록에는 없다.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        via: Option<Via>,
        /// 관문 동사가 보낸 메시지의 관문 흔적. 다른 메시지에는 없다.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        gate: Option<GateTrace>,
    },
    /// 보내기를 거절했다. `to`는 보낸 쪽이 적은 이름이다. 관문 호출이면 브랜치다.
    Refused {
        how: Kind,
        from: Who,
        to: String,
        reason: Refusal,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        via: Option<Via>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        gate: Option<GateTrace>,
    },
    /// 공급자의 쓰는 계정을 바꾸고 실행 중인 노드를 옮겼다. 계정이 없으면 시스템 기본이다.
    /// `moved`는 바로 옮긴 노드, `later`는 턴이 끝나면 옮길 노드, `failed`는 옛 계정으로 계속 도는 노드 수다.
    /// 목업 docs/mockups/account-switch.html ③.
    AccountSwitched {
        provider: Provider,
        /// 계정의 위치(WSL 배포판). 이 PC면 없다. 옛 기록에도 없다.
        #[serde(default)]
        distro: Option<String>,
        from: Option<AccountId>,
        to: Option<AccountId>,
        why: SwitchCause,
        moved: u32,
        later: u32,
        failed: u32,
    },
    /// 노드를 다시 띄웠다(「다시 띄우기」). `error`가 있으면 옛 프로세스로 계속 돌거나 프로세스 없이 남았다.
    /// 턴 끝에 다시 띄운 결과를 사람이 여기서 본다. 화면은 이 기록으로 「턴이 끝나면 다시 띄움」을 거둔다.
    Relaunched {
        node: Who,
        error: Option<String>,
    },
    /// 자동 전환할 계정이 없어 바꾸지 않았다. `resets_at`은 그 공급자 계정 가운데 가장 빠른 창 초기화(유닉스 초)다.
    NoAccountLeft {
        provider: Provider,
        /// 계정의 위치(WSL 배포판). 이 PC면 없다. 옛 기록에도 없다.
        #[serde(default)]
        distro: Option<String>,
        #[ts(type = "number | null")]
        resets_at: Option<i64>,
    },
    /// 사람이 띄운 노드가 `loom ask`로 사람에게 물었다. `text`는 질문 전문이다.
    Asked {
        node: Who,
        wait: WaitId,
        text: String,
        /// 붙은 문서(`--doc`). 질문이 닫힌 뒤에도 이벤트 콘솔에서 다시 연다(목업 docs/mockups/doc-reopen.html M13 ①). 옛 기록에는 없다.
        #[serde(default)]
        doc: Option<WaitDoc>,
    },
    /// 사람이 띄운 노드가 `loom done`으로 받은편지함에 완료를 걸었다. `instruction`은 `--asked`로 받은 사람의 지시다.
    Reported {
        node: Who,
        wait: WaitId,
        summary: String,
        instruction: String,
        /// 붙은 문서(`--doc`). `Asked`와 같다.
        #[serde(default)]
        doc: Option<WaitDoc>,
    },
    /// 사람이 받은편지함에서 질문에 답했다. `images`는 답에 붙인 그림 수다.
    Answered {
        node: Who,
        wait: WaitId,
        text: String,
        images: u32,
    },
    /// 사람이 받은편지함의 완료 줄에서 이어서 맡겼다. 그 완료는 닫혔다.
    Instructed {
        node: Who,
        wait: WaitId,
        text: String,
        images: u32,
    },
    /// 질문이 답 없이 닫혔다.
    Unanswered {
        node: Who,
        wait: WaitId,
        by: ClosedBy,
    },
    /// 사람이 받은편지함의 완료를 [확인]했다.
    Confirmed {
        node: Who,
        wait: WaitId,
    },
}

/// 요청이 들어온 입구. CLI는 명령 이름(`loom note`)과 판을 헤더로 보낸다. 헤더가 없는 요청(화면, 훅 밖 API)은 경로다.
/// 근거: R8 v5 ⑤ 흔적 칸 「입구」·「판」. 관문 audit가 관문 동사 밖에서 온 관문 글을 이 값으로 가린다.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Via {
    pub tool: String,
    /// loom 판과 (있으면) 커밋. 헤더가 없으면 없다.
    pub version: Option<String>,
}

/// 관문 호출 하나의 흔적. 누가(역할·실행자·모델) 어느 브랜치에 무엇을 했고 상태가 어떻게 바뀌었는지.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GateTrace {
    pub verb: GateVerb,
    pub branch: String,
    /// 부르기 전 상태. 브랜치를 찾기 전에 거절했으면 없다.
    pub from: Option<GateState>,
    /// 거절이면 없다.
    pub to: Option<GateState>,
    pub role: Option<String>,
    pub agent: Option<AgentId>,
    pub model: Option<String>,
    /// `loom merge` 안 audit가 찾은 일. 다른 호출은 빈 목록이다.
    pub findings: Vec<Finding>,
}

/// 질문이 답 없이 닫힌 까닭.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ClosedBy {
    /// 사람이 받은편지함에서 [×]를 눌렀다.
    Human,
    /// 에이전트가 `loom answered`로 닫았다. 사람이 터미널에서 답했다고 판단했다.
    Agent,
    /// 노드 프로세스를 다시 띄웠다.
    Restart,
    /// 노드 프로세스가 끝났다.
    Exit,
}

/// 계정을 바꾼 이유.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum SwitchCause {
    /// 사람이 [모두 바꾸기]를 눌렀다.
    Manual,
    /// 자동 전환: 노드가 한도 오류로 멈췄다.
    RateLimit,
}

#[derive(Clone, Debug, Serialize, TS)]
#[ts(export)]
pub struct EventRecord {
    /// 1씩 늘어난다. 화면은 건너뛴 번호로 놓친 이벤트를 안다.
    pub id: u32,
    /// 유닉스 초.
    #[ts(type = "number")]
    pub at: i64,
    pub entry: Entry,
    /// 메시지 이벤트의 본문. 메시지 테이블에서 읽는다. 메시지를 지웠으면 없다. 화면은 `parts`를 그린다.
    #[serde(skip)]
    pub body: Option<String>,
    /// 줄을 펴면 보이는 글의 조각: 메시지 본문, 질문·완료·사람 답의 글. 없으면 빈 목록이다.
    pub parts: Vec<TextPart>,
    /// `ask` 메시지의 표준 꼴 선택지. 화면은 단추 모양으로 보인다(목업 M17 ③).
    pub choices: Vec<Choice>,
    /// 메시지 이벤트만.
    pub message: Option<MessageFacts>,
}

/// 메시지 이벤트 줄이 보이는 사실. 종류가 없는 옛 메시지는 `by_loom`이 거짓이고 칸이 없다.
#[derive(Clone, Debug, Serialize, TS)]
#[ts(export)]
pub struct MessageFacts {
    /// 메시지 번호(`#2925`). `answer`의 `question` 칸이 이 번호를 가리킨다.
    pub id: u32,
    /// loom이 보낸 알림이다. 화면은 보낸 쪽을 「loom」으로 보인다.
    pub by_loom: bool,
    pub fields: Vec<FieldParts>,
}

/// 종류의 칸 하나와 그 값의 조각. 파일 칸은 링크 하나다.
#[derive(Clone, Debug, Serialize, TS)]
#[ts(export)]
pub struct FieldParts {
    pub name: Field,
    pub parts: Vec<TextPart>,
}

impl EventRecord {
    fn new(id: u32, at: i64, entry: Entry, message: Option<u32>, body: Option<String>) -> Self {
        let text = match &entry {
            Entry::Message { .. } => body.as_deref(),
            Entry::Asked { text, .. }
            | Entry::Answered { text, .. }
            | Entry::Instructed { text, .. } => Some(text.as_str()),
            Entry::Reported { summary, .. } => Some(summary.as_str()),
            _ => None,
        };
        let question = match &entry {
            Entry::Message {
                what: Some(MessageKind::Ask),
                ..
            } => text,
            _ => None,
        };
        let message = match (&entry, message) {
            (Entry::Message { what, .. }, Some(id)) => Some(MessageFacts {
                id,
                by_loom: what.as_ref().is_some_and(MessageKind::by_loom),
                // 판정은 줄 머리의 종류 이름이 보인다(「판정 · 막음」, 목업 M17 ①).
                fields: what
                    .iter()
                    .flat_map(MessageKind::fields)
                    .filter(|(name, _)| *name != Field::Judgment)
                    .map(|(name, value)| FieldParts {
                        name,
                        parts: if name.is_file() {
                            vec![path_link(&value)]
                        } else {
                            parts(&value)
                        },
                    })
                    .collect(),
            }),
            _ => None,
        };
        EventRecord {
            id,
            at,
            parts: text.map(parts).unwrap_or_default(),
            choices: question.map(choices).unwrap_or_default(),
            message,
            entry,
            body,
        }
    }
}

const RECORD_COLUMNS: &str = "SELECT e.id, e.at, e.entry, m.body, e.message FROM events e
    LEFT JOIN messages m ON m.id = e.message";

/// `message`는 메시지 이벤트가 가리키는 메시지다. 본문을 복사하지 않는다.
pub(super) fn write(
    db: &Connection,
    entry: &Entry,
    message: Option<MessageId>,
) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO events (at, entry, message) VALUES (unixepoch(), ?1, ?2)",
        params![to_json(entry), message.map(|m| m.0)],
    )?;
    db.execute(
        "DELETE FROM events WHERE id <= ?1",
        [db.last_insert_rowid() - KEEP as i64],
    )?;
    Ok(())
}

/// 가장 오래 남은 이벤트의 시각. 관문 audit가 흔적 구간이 빈틈없는지 본다.
pub(super) fn oldest_at(db: &Connection) -> rusqlite::Result<Option<i64>> {
    db.query_row("SELECT min(at) FROM events", [], |row| row.get(0))
}

/// `since`(유닉스 초) 뒤에 보낸 메시지 이벤트와 그 본문, 오래된 것부터. 관문 audit가 읽는다.
pub(super) fn sent_since(db: &Connection, since: i64) -> rusqlite::Result<Vec<EventRecord>> {
    db.prepare(&format!(
        "{RECORD_COLUMNS} WHERE e.at >= ?1 AND e.entry ->> 'kind' = 'message' AND e.entry ->> 'step' = 'sent' ORDER BY e.id"
    ))?
    .query_map([since], record_from_row)?
    .collect()
}

pub(super) fn who(db: &Connection, id: NodeId) -> rusqlite::Result<Who> {
    Ok(Who {
        id: Some(id),
        name: super::name_of(db, id)?,
    })
}

/// 최근 것부터.
pub(super) fn recent(db: &Connection) -> rusqlite::Result<Vec<EventRecord>> {
    db.prepare(&format!("{RECORD_COLUMNS} ORDER BY e.id DESC LIMIT ?1"))?
        .query_map([RECENT], record_from_row)?
        .collect()
}

/// `after`보다 뒤의 이벤트, 오래된 것부터.
pub(super) fn after(db: &Connection, after: u32) -> rusqlite::Result<Vec<EventRecord>> {
    db.prepare(&format!("{RECORD_COLUMNS} WHERE e.id > ?1 ORDER BY e.id"))?
        .query_map([after], record_from_row)?
        .collect()
}

pub(super) fn last_id(db: &Connection) -> rusqlite::Result<u32> {
    let id: Option<u32> = db.query_row("SELECT max(id) FROM events", [], |row| row.get(0))?;
    Ok(id.unwrap_or(0))
}

fn record_from_row(row: &Row) -> rusqlite::Result<EventRecord> {
    Ok(EventRecord::new(
        row.get(0)?,
        row.get(1)?,
        from_json(row, 2)?,
        row.get(4)?,
        row.get(3)?,
    ))
}
