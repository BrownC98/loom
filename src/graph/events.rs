use rusqlite::{Connection, Row, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::agent_state::{AgentEvent, AgentState, ShadowCheck};
use super::{NodeId, NodeState, from_json, to_json};
use crate::accounts::AccountId;
use crate::choices::{Choice, choices};
use crate::link_text::{TextPart, parts, path_link};
use crate::protocol::{
    AgentId, Field, Finding, GateState, GateVerb, Kind, MessageId, MessageKind, Provider, Refusal,
    WaitDoc, WaitId,
};

const KEEP: u32 = 10_000;
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
    /// 에이전트 동사를 거절했다. 동사는 `via`의 명령 이름이다. `to`는 부른 쪽이 적은 대상(노드 이름, 메시지 번호)이다. 관문 호출이면 브랜치다.
    Refused {
        how: Kind,
        /// 보내려던 종류(`note`·`answer`·`done`·`ask`). 메시지가 아닌 동사(`spawn`·`rm` 등), 관문 호출과 옛 기록에는 없다.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        what: Option<MessageKind>,
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
        /// 다른 에이전트로 바꿔 띄웠다(`loom relaunch --agent`). 같은 세션을 이었으면, 그리고 옛 기록에는 없다.
        #[serde(default)]
        switched: Option<AgentSwitch>,
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
    /// 턴 끝 검사가 노드를 한 번 다시 일하게 했다. 발동 수를 재려고 남긴다(R14 5절, 2026-10-23 Q687 A).
    Nudged {
        node: Who,
        check: TurnCheck,
    },
    /// 병합 큐의 쌓기·검사 한 번(2026-10-23 사람 결정 Q697 A). 브랜치마다 끝은 관문 메시지(`gate` 흔적)로 따로 남는다.
    Batch {
        #[ts(type = "number")]
        batch: i64,
        /// 원래 체크아웃의 저장소 폴더.
        repo: String,
        target: String,
        /// 0이면 묶음 전체, 1부터 이분 탐색 단계다.
        step: u32,
        /// 쌓은 순서(큐 순서). 크기는 이 목록의 길이다.
        members: Vec<BatchMember>,
        /// check.sh가 걸린 초. 검사 전에 끝났으면 없다.
        #[ts(type = "number | null")]
        check_secs: Option<u64>,
        result: BatchResult,
        log: Option<String>,
    },
    /// 쉬는 노드의 훅이 메시지를 기다리기 시작했다(`/api/agent/idle`). 화면 이벤트 콘솔은 보이지 않는다.
    IdleOpened {
        node: Who,
    },
    /// 쉬는 대기가 끝났다. 화면 이벤트 콘솔은 보이지 않는다.
    IdleEnded {
        node: Who,
        by: IdleEnd,
    },
    /// 턴 끝 답 글(`said`) 뒤 `IDLE_GRACE` 안에 stop 훅이 데몬에 닿지 않았다. 데몬이 노드를 같은 세션으로 다시 띄운다. 그 결과는 다음 `Relaunched`다. 화면 이벤트 콘솔은 보이지 않는다.
    NoIdleWait {
        node: Who,
    },
    /// 에이전트 상태가 바뀌었다(`AgentState`). 화면 이벤트 콘솔은 보이지 않는다.
    AgentStateChanged {
        node: Who,
        from: AgentState,
        to: AgentState,
        by: AgentEvent,
    },
    /// 옛 판정(`check`의 값 `old`)과 상태 기계(`state`)가 달랐다. 상태가 바뀔 때까지 한 번만 남긴다. 화면 이벤트 콘솔은 보이지 않는다.
    AgentStateDisagreed {
        node: Who,
        check: ShadowCheck,
        old: bool,
        state: AgentState,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BatchMember {
    pub branch: String,
    pub head: String,
    /// pass 판정부터 이 검사 시작까지 기다린 초.
    #[ts(type = "number")]
    pub waited_secs: u64,
}

/// 묶음 검사 한 번의 결과.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum BatchResult {
    /// check.sh가 지났다. 다음 일은 게시다.
    Passed,
    CheckFailed,
    /// 이 브랜치를 쌓다 충돌했다. 그 브랜치는 반려되고 나머지로 다시 쌓는다.
    Conflict {
        branch: String,
    },
    /// 게시 전에 대상이 움직였다. 모두 큐에 다시 선다.
    TargetMoved,
    /// 대상 부모 워크트리에 미커밋 변경이 있다. 모두 반려된다.
    TargetDirty,
    /// 데몬이 묶음 중에 끝났다. 다시 띄운 데몬이 거뒀다.
    Interrupted,
    /// 스크립트를 띄우지 못했거나 예상 밖 끝 코드다.
    Error {
        cause: String,
    },
}

/// 쉬는 대기가 끝난 까닭.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum IdleEnd {
    /// 대기가 메시지를 꺼내 훅에 넘겼다.
    Mail,
    /// 같은 노드의 새 쉬는 대기가 열렸다.
    NewWait,
    /// 턴 안의 훅이 메일함을 읽었다. `hook`은 런타임이 준 훅 이름이다.
    Turn { hook: String },
    /// 노드를 다시 띄웠거나, 다시 띄우기가 실패해 대기를 풀었다.
    Relaunch,
    /// 노드 프로세스가 끝났다.
    Exit,
    /// 완료 노드를 재웠다.
    Sleep,
    /// 끝나기 전에 요청이 사라졌다. 훅 프로세스가 끝났거나 연결이 끊겼다.
    Gone,
    /// 메일함을 읽지 못했다.
    Error,
}

/// 다시 일하게 한 턴 끝 검사(`daemon::turn_end`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TurnCheck {
    /// 사람이 띄운 노드가 사람 턴에 글을 쓰고 `loom ask`도 `loom done`도 없이 끝냈다.
    Unreported,
    /// 종류 낱말로 시작하는 줄을 쓰고 loom 동사를 부르지 않았다.
    KindLine,
    /// 런타임의 자체 질문 창(Cursor AskQuestion)으로 물었다. 사람은 `loom ask`로만 답한다.
    QuestionBox,
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
    /// 병합 큐의 audit가 찾은 일. 다른 호출은 빈 목록이다.
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
    /// 노드 프로세스를 다시 띄웠다. 다시 띄우기가 질문을 남기기 전의 옛 기록에만 있다.
    Restart,
    /// 노드 프로세스가 끝났다.
    Exit,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentSwitch {
    pub from: AgentId,
    pub to: AgentId,
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

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct EventLog {
    /// 숨긴 이벤트까지 센 마지막 번호. 화면은 이 번호 다음부터 건너뛴 번호를 센다.
    pub last: u32,
    /// 화면에 보이는 최근 이벤트. 새것부터. 쉬는 대기의 열림·끝은 없다.
    pub records: Vec<EventRecord>,
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
    /// 화면이 보낸 쪽을 「loom」으로 보이고, 전달 단계 대신 칸 요약을 보인다(목업 M17 ③ 「보내는 쪽」).
    pub from_loom: bool,
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
            Entry::Message { what, .. } => body
                .as_deref()
                .filter(|body| !what.as_ref().is_some_and(|w| w.repeats_file(body))),
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
                from_loom: what.as_ref().is_some_and(shown_from_loom),
                fields: screen_fields(what.as_ref()),
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

fn screen_fields(what: Option<&MessageKind>) -> Vec<FieldParts> {
    let mut fields: Vec<FieldParts> = Vec::new();
    for (name, value) in what.iter().flat_map(|w| w.fields()) {
        match (name, fields.last_mut()) {
            (Field::Judgment, _) => {}
            (Field::Head, Some(branch)) if branch.name == Field::Branch => {
                branch.parts.push(TextPart::Text {
                    text: format!(" · {value}"),
                });
            }
            _ => fields.push(FieldParts {
                name,
                parts: if name.is_file() {
                    vec![path_link(&value)]
                } else {
                    parts(&value)
                },
            }),
        }
    }
    fields
}

fn shown_from_loom(what: &MessageKind) -> bool {
    match what {
        MessageKind::Merged { .. }
        | MessageKind::Withdrawn { .. }
        | MessageKind::Rework { .. }
        | MessageKind::Failed { .. }
        | MessageKind::Exited { .. }
        | MessageKind::SetupFailed { .. }
        | MessageKind::Removed { .. }
        | MessageKind::Adopted { .. }
        | MessageKind::RulesChanged { .. } => true,
        MessageKind::Ask
        | MessageKind::Answer { .. }
        | MessageKind::Done { .. }
        | MessageKind::Note
        | MessageKind::Ready { .. }
        | MessageKind::Verdict { .. }
        | MessageKind::HumanInput { .. } => false,
    }
}

const RECORD_COLUMNS: &str = "SELECT e.id, e.at, e.entry, m.body, e.message FROM events e
    LEFT JOIN messages m ON m.id = e.message";

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

pub(super) fn oldest_at(db: &Connection) -> rusqlite::Result<Option<i64>> {
    db.query_row("SELECT min(at) FROM events", [], |row| row.get(0))
}

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

pub(super) fn log(db: &Connection) -> rusqlite::Result<EventLog> {
    let records = db
        .prepare(&format!(
            "{RECORD_COLUMNS} WHERE e.entry ->> 'kind' NOT IN ('idleOpened', 'idleEnded', 'noIdleWait', 'agentStateChanged', 'agentStateDisagreed') ORDER BY e.id DESC LIMIT ?1"
        ))?
        .query_map([RECENT], record_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(EventLog {
        last: last_id(db)?,
        records,
    })
}

#[cfg(test)]
pub(super) fn recent(db: &Connection) -> rusqlite::Result<Vec<EventRecord>> {
    db.prepare(&format!("{RECORD_COLUMNS} ORDER BY e.id DESC LIMIT ?1"))?
        .query_map([RECENT], record_from_row)?
        .collect()
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn message(what: MessageKind, body: &str) -> EventRecord {
        let entry = Entry::Message {
            how: Kind::Queue,
            what: Some(what),
            from: Who {
                id: Some(NodeId(2)),
                name: "gate".into(),
            },
            to: None,
            step: Step::Delivered,
            via: None,
            gate: None,
        };
        EventRecord::new(1, 0, entry, Some(7), Some(body.into()))
    }

    #[test]
    fn merge_notices_are_from_loom_and_human_input_is_from_the_node() {
        let merged = MessageKind::Merged {
            branch: "b".into(),
            head: "h".into(),
            sha: "s".into(),
            log: None,
        };
        assert!(message(merged, "").message.unwrap().from_loom);
        let typed = MessageKind::HumanInput { node: "n".into() };
        assert!(!message(typed, "hi").message.unwrap().from_loom);
    }

    #[test]
    fn verdict_fields_join_the_commit_to_the_branch_and_drop_the_judgment() {
        let verdict = MessageKind::Verdict {
            branch: "b".into(),
            head: "3f2a91c".into(),
            judgment: crate::protocol::Judgment::Block,
            findings: None,
            reviewer: Some("claude opus".into()),
        };
        let fields = message(verdict, "").message.unwrap().fields;
        let names: Vec<Field> = fields.iter().map(|f| f.name).collect();
        assert_eq!(names, [Field::Branch, Field::Reviewer]);
        assert_eq!(fields[0].parts.len(), 2);
    }

    #[test]
    fn a_done_with_only_a_file_shows_the_path_once() {
        let done = |file: &str| MessageKind::Done {
            file: Some(file.into()),
            warning: None,
        };
        assert_eq!(message(done("/r.md"), "/r.md").parts, []);
        assert_eq!(message(done("/r.md"), "summary").parts.len(), 1);
    }
}
