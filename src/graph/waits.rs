use std::fmt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::events::{self, ClosedBy, Entry};
use super::mailbox;
use crate::choices::{Choice, choices, offers_choices_without_tail};
use crate::link_text::{TextPart, bare_link, parts};
use crate::profile::AgentId;
use crate::protocol::{MessageKind, Question};

use super::{Graph, NodeId, NodeState, Refusal, from_json, parent_of, state_of, to_json};

/// 사람이 할 일의 종류. 화면이 이름과 아이콘을 고른다.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum WaitKind {
    /// 에이전트가 고르기·답을 묻는다.
    Question,
    /// 명령·도구·계획을 승인해야 한다.
    Approval,
    /// 폴더 신뢰를 확인해야 한다.
    Trust,
    /// 로그인이나 계정 한도 선택이 필요하다.
    Auth,
    /// 터미널 프로그램이 입력(비밀번호, y/n)을 기다린다.
    Input,
    /// 사람이 띄운 노드가 맡은 일을 끝냈다(`loom done --asked`). 사람은 [확인]하거나 이어서 맡긴다.
    Done,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WaitSource {
    Hook,
    Codex,
    Screen,
}

/// 대기 번호. 사람은 `Q12`로 읽고 쓴다. 데몬 전체에서 늘기만 하고(DB `wait_ids`) 다시 쓰지 않는다.
/// 근거: 2026-10-21 사람 지시 "질문에 id 부여하고 복사하기 편하게 만들어서".
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WaitId(pub u32);

impl fmt::Display for WaitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Q{}", self.0)
    }
}

/// 질문·완료에 붙은 문서(user-doc 목업·보고서 HTML). 화면은 ID와 제목만 받고 `/api/docs/{id}/`로 연다. 파일 경로는 데몬의 문서 ID 표(`user_doc::doc_file`)만 안다.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WaitDoc {
    /// `M25`, `R3` 같은 문서 ID.
    pub id: String,
    pub title: String,
}

/// 터미널 승인 창의 본문과 선택지(`screen::prompt`). 화면은 선택지마다 버튼을 그리고, 누르면 이 값을 그대로 돌려보낸다(`Graph::approval`).
/// 데몬은 보내기 직전 화면을 다시 읽어 같은 값일 때만 키를 친다. 그래서 다른 창에 키가 들어가지 않는다(2026-10-23 목업 M11 승인).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Prompt {
    /// 창 위부터 마지막 선택지까지 화면 줄 그대로. 같은 창인지는 이 줄들이 정한다. `body`는 보기 좋게 다듬은 글이라 다른 명령이 같은 글이 될 수 있다(2026-10-23 리뷰: 명령 줄 「Tip:; touch one」).
    pub rows: Vec<String>,
    /// 창 본문(도구, 명령, 이유) 줄들. 명령 줄은 앞 공백까지 터미널 글 그대로다.
    pub body: Vec<String>,
    /// 무엇을 승인하는지 한 줄(명령). 알약 미리 보기가 쓴다.
    pub summary: String,
    pub options: Vec<PromptOption>,
}

/// 승인 창의 선택지 하나. `label`은 터미널 글(영어) 그대로다.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PromptOption {
    pub label: String,
    pub key: PromptKey,
}

/// 선택지를 고르는 키 하나. 누르면 바로 정한다(Claude 2.1.309, Codex 0.159.2 실측: 숫자 키 하나, Enter 없음).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
#[ts(export)]
pub enum PromptKey {
    Char(char),
    Tab,
    BackTab,
}

impl PromptKey {
    pub fn bytes(self) -> Vec<u8> {
        match self {
            PromptKey::Char(c) => c.to_string().into_bytes(),
            PromptKey::Tab => b"\t".to_vec(),
            PromptKey::BackTab => b"\x1b[Z".to_vec(),
        }
    }
}

/// 화면에 보내는 대기 한 건.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HumanWait {
    pub id: WaitId,
    pub kind: WaitKind,
    /// `loom ask` 질문과 완료는 전문, 다른 대기는 한 줄이다. 없으면 빈 글이고, 화면은 종류 이름만 보인다.
    pub text: String,
    /// 대기가 시작된 유닉스 초.
    #[ts(type = "number")]
    pub since: i64,
    /// `loom ask` 질문이나 완료다. 화면에서 답할 수 있다(`Graph::answer`). 완료의 답은 이어서 맡기는 일이다. 다른 대기는 그 노드 터미널에서만 답한다.
    pub answerable: bool,
    pub doc: Option<WaitDoc>,
    /// 완료(`Done`)만: 그 일을 맡긴 사람의 지시(`loom done --asked`). `text`는 결과(`loom done`의 요약)다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub instruction: Option<String>,
    /// 완료(`Done`)만: 사람이 그 지시에 준 그림의 데몬 기계 경로(`loom done --image`). 화면은 `GET /api/waits/{id}/images/{index}`로 읽는다.
    #[serde(default)]
    pub images: Vec<String>,
    /// 승인(`Approval`) 신호 대기만: 지금 터미널 화면에서 읽은 승인 창. 읽지 못했으면 없다. DB에 두지 않는다(`Graph::set_prompt`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub prompt: Option<Prompt>,
    /// `text`의 글 조각. 화면에 보낼 때만 채운다(`for_screen`). DB에 두지 않는다.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<TextPart>,
    /// 질문(`Question`)만: 표준 꼴 선택지. `parts`와 같이 화면에 보낼 때만 채운다.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<Choice>,
}

impl HumanWait {
    pub(super) fn for_screen(self) -> HumanWait {
        HumanWait {
            parts: parts(&self.text),
            choices: match self.kind {
                WaitKind::Question => choices(&self.text),
                WaitKind::Approval
                | WaitKind::Trust
                | WaitKind::Auth
                | WaitKind::Input
                | WaitKind::Done => Vec::new(),
            },
            ..self
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct Stored {
    #[serde(flatten)]
    pub wait: HumanWait,
    pub source: WaitSource,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct OpenQuestion {
    pub id: WaitId,
    pub text: String,
}

const TEXT_LIMIT_CHARS: usize = 200;
const QUESTION_LIMIT_CHARS: usize = 1000;
pub const HUMAN: &str = "human";

impl Graph {
    pub fn set_wait(
        &self,
        token: &str,
        source: WaitSource,
        wait: Option<(WaitKind, &str)>,
    ) -> rusqlite::Result<()> {
        let db = self.lock();
        let Some(id) = super::nodes::caller_in(&db, token)? else {
            return Ok(());
        };
        let Some(now) = stored(&db, id)? else {
            return Ok(());
        };
        let next = match wait {
            Some((kind, text)) => {
                let text = one_line(text);
                if let Some(now) = &now
                    && now.wait.kind == kind
                    && now.wait.text == text
                {
                    return Ok(());
                }
                Some(Stored {
                    wait: new_wait(&db, kind, text, false, None)?,
                    source,
                })
            }
            None if now.as_ref().is_some_and(|n| n.source == source) => None,
            None => return Ok(()),
        };
        put(&db, id, next.as_ref())?;
        self.publish_node(&db, id)
    }

    pub fn ask(&self, id: NodeId, question: &str, doc: Option<WaitDoc>) -> Result<WaitId, Refusal> {
        let question = question.trim();
        if question.is_empty() {
            return Err(Refusal::EmptyQuestion);
        }
        let text: String = question.chars().take(QUESTION_LIMIT_CHARS).collect();
        let malformed = bare_link(&text)
            .map(Refusal::BareLink)
            .or_else(|| offers_choices_without_tail(&text).then_some(Refusal::BadChoices));
        let mut db = self.lock();
        if let Some(refusal) = malformed {
            return Err(refusal);
        }
        let tx = db.transaction()?;
        let wait = new_wait(&tx, WaitKind::Question, text.clone(), true, doc.clone())?;
        let asked = wait.id;
        tx.execute(
            "INSERT INTO questions (id, node, wait) VALUES (?1, ?2, ?3)",
            params![asked.0, id.0, to_json(&wait)],
        )?;
        let entry = Entry::Asked {
            node: events::who(&tx, id)?,
            wait: asked,
            text,
            doc,
        };
        events::write(&tx, &entry, None)?;
        tx.commit()?;
        self.publish_node(&db, id)?;
        Ok(asked)
    }

    pub fn answer(&self, wait: WaitId, text: &str, images: &[PathBuf]) -> Result<NodeId, Refusal> {
        let text = text.trim();
        if text.is_empty() && images.is_empty() {
            return Err(Refusal::EmptyAnswer);
        }
        let mut db = self.lock();
        let count = images.len() as u32;
        let images = match images {
            [] => String::new(),
            paths => {
                let quoted: Vec<String> =
                    paths.iter().map(|p| quote(&p.to_string_lossy())).collect();
                format!("\nImages: {}", quoted.join(" "))
            }
        };
        type Record = fn(events::Who, WaitId, String, u32) -> Entry;
        let (node, what, body, record): (NodeId, MessageKind, String, Record) = match open_ask(
            &db, wait,
        ) {
            Ok((node, question)) => (
                node,
                MessageKind::Answer {
                    question: Question::Inbox { id: wait.0 },
                },
                format!(
                    "Answer to loom ask question {wait} ({}): {text}{images}\nThe human answered from the loom inbox. The question is closed: do not call loom answered.",
                    question.text
                ),
                |node, wait, text, images| Entry::Answered {
                    node,
                    wait,
                    text,
                    images,
                },
            ),
            Err(Refusal::NoWait(_)) => (
                done_node(&db, wait)?,
                MessageKind::Note,
                format!(
                    "New instruction from the human in the loom inbox, after your done report: {text}{images}"
                ),
                |node, wait, text, images| Entry::Instructed {
                    node,
                    wait,
                    text,
                    images,
                },
            ),
            Err(e) => return Err(e),
        };
        let tx = db.transaction()?;
        if matches!(what, MessageKind::Answer { .. }) {
            tx.execute("INSERT OR IGNORE INTO answers (id) VALUES (?1)", [wait.0])?;
        }
        mailbox::enqueue_human(&tx, node, &what, &body)?;
        tx.execute("DELETE FROM questions WHERE id = ?1", [wait.0])?;
        tx.execute("DELETE FROM dones WHERE id = ?1", [wait.0])?;
        let entry = record(events::who(&tx, node)?, wait, text.to_owned(), count);
        events::write(&tx, &entry, None)?;
        tx.commit()?;
        self.publish_node(&db, node)?;
        self.mail.notify_waiters();
        Ok(node)
    }

    pub fn answered(&self, id: NodeId, ids: &[WaitId]) -> Result<Vec<WaitId>, Refusal> {
        let mut db = self.lock();
        let open: Vec<WaitId> = questions(&db, id)?.iter().map(|q| q.id).collect();
        let closing = match ids {
            [] if open.len() > 1 => return Err(Refusal::SeveralOpen(open)),
            [] => open,
            ids => {
                if let Some(missing) = ids.iter().find(|q| !open.contains(q)) {
                    return Err(Refusal::NoWait(*missing));
                }
                ids.to_vec()
            }
        };
        let tx = db.transaction()?;
        for q in &closing {
            tx.execute("DELETE FROM questions WHERE id = ?1", [q.0])?;
            unanswered(&tx, id, *q, ClosedBy::Agent)?;
        }
        tx.commit()?;
        if !closing.is_empty() {
            self.publish_node(&db, id)?;
        }
        Ok(closing)
    }

    pub fn unreported_root(&self, id: NodeId) -> Result<bool, Refusal> {
        let db = self.lock();
        if parent_of(&db, id)?.is_some() || state_of(&db, id)? != NodeState::Running {
            return Ok(false);
        }
        Ok(stored(&db, id)?.is_some_and(|wait| wait.is_none()) && questions(&db, id)?.is_empty())
    }

    pub fn answered_in_inbox(&self, wait: WaitId) -> rusqlite::Result<bool> {
        self.lock().query_row(
            "SELECT EXISTS (SELECT 1 FROM answers WHERE id = ?1)",
            [wait.0],
            |row| row.get(0),
        )
    }

    pub fn open_questions(&self, id: NodeId) -> Result<Vec<OpenQuestion>, Refusal> {
        Ok(questions(&self.lock(), id)?
            .into_iter()
            .map(|q| OpenQuestion {
                id: q.id,
                text: q.text,
            })
            .collect())
    }

    pub fn typed(&self, id: NodeId) -> rusqlite::Result<()> {
        let db = self.lock();
        let changed = db.execute(
            "UPDATE nodes SET wait = NULL WHERE id = ?1 AND wait IS NOT NULL",
            [id.0],
        )?;
        if changed > 0 {
            self.publish_node(&db, id)?;
        }
        Ok(())
    }

    pub fn dismiss(&self, wait: WaitId) -> Result<(), Refusal> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let node: Option<u32> = tx
            .query_row(
                "DELETE FROM questions WHERE id = ?1 RETURNING node",
                [wait.0],
                |row| row.get(0),
            )
            .optional()?;
        let asked = node.is_some();
        let node = match node.map(NodeId) {
            Some(node) => {
                mailbox::enqueue_human(
                    &tx,
                    node,
                    &MessageKind::Note,
                    &format!("{wait} was closed without an answer."),
                )?;
                unanswered(&tx, node, wait, ClosedBy::Human)?;
                node
            }
            None => match done_node(&tx, wait) {
                Ok(node) => {
                    tx.execute("DELETE FROM dones WHERE id = ?1", [wait.0])?;
                    let entry = Entry::Confirmed {
                        node: events::who(&tx, node)?,
                        wait,
                    };
                    events::write(&tx, &entry, None)?;
                    node
                }
                Err(Refusal::NoWait(_)) => tx
                    .query_row(
                        "UPDATE nodes SET wait = NULL WHERE wait ->> 'id' = ?1 RETURNING id",
                        [wait.0],
                        |row| row.get(0),
                    )
                    .optional()?
                    .map(NodeId)
                    .ok_or(Refusal::NoWait(wait))?,
                Err(e) => return Err(e),
            },
        };
        tx.commit()?;
        if asked {
            self.mail.notify_waiters();
        }
        Ok(self.publish_node(&db, node)?)
    }

    pub fn done_image(&self, wait: WaitId, index: usize) -> Result<PathBuf, Refusal> {
        let done: Option<String> = self
            .lock()
            .query_row("SELECT wait FROM dones WHERE id = ?1", [wait.0], |row| {
                row.get(0)
            })
            .optional()?;
        done.and_then(|d| serde_json::from_str::<HumanWait>(&d).ok())
            .and_then(|d| d.images.into_iter().nth(index))
            .map(PathBuf::from)
            .ok_or(Refusal::NoWait(wait))
    }

    pub fn approval(&self, wait: WaitId) -> Result<(NodeId, Option<AgentId>), Refusal> {
        let found: Option<(u32, Option<AgentId>)> = self
            .lock()
            .query_row(
                "SELECT id, agent FROM nodes WHERE wait ->> 'id' = ?1 AND wait ->> 'kind' = 'approval'",
                [wait.0],
                |row| Ok((row.get(0)?, from_json(row, 1)?)),
            )
            .optional()?;
        let (node, agent) = found.ok_or(Refusal::NoWait(wait))?;
        Ok((NodeId(node), agent))
    }
}

pub(super) fn put_done(
    db: &rusqlite::Connection,
    id: NodeId,
    asked: &str,
    summary: &str,
    images: &[PathBuf],
    doc: Option<WaitDoc>,
) -> rusqlite::Result<()> {
    let summary = summary.trim().to_owned();
    let instruction: String = asked.chars().take(QUESTION_LIMIT_CHARS).collect();
    let mut wait = new_wait(db, WaitKind::Done, summary.clone(), true, doc.clone())?;
    wait.instruction = Some(instruction.clone());
    wait.images = images
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let done = wait.id;
    db.execute(
        "INSERT INTO dones (id, node, wait) VALUES (?1, ?2, ?3)",
        params![done.0, id.0, to_json(&wait)],
    )?;
    let entry = Entry::Reported {
        node: events::who(db, id)?,
        wait: done,
        summary,
        instruction,
        doc,
    };
    events::write(db, &entry, None)
}

pub(super) fn drop_questions(db: &rusqlite::Connection, id: NodeId) -> rusqlite::Result<()> {
    let closed = db
        .prepare("DELETE FROM questions WHERE node = ?1 RETURNING id")?
        .query_map([id.0], |row| row.get(0).map(WaitId))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for wait in &closed {
        unanswered(db, id, *wait, ClosedBy::Exit)?;
    }
    Ok(())
}

fn unanswered(
    db: &rusqlite::Connection,
    id: NodeId,
    wait: WaitId,
    by: ClosedBy,
) -> rusqlite::Result<()> {
    let entry = Entry::Unanswered {
        node: events::who(db, id)?,
        wait,
        by,
    };
    events::write(db, &entry, None)
}

fn stored(db: &rusqlite::Connection, id: NodeId) -> rusqlite::Result<Option<Option<Stored>>> {
    let text: Option<Option<String>> = db
        .query_row("SELECT wait FROM nodes WHERE id = ?1", [id.0], |row| {
            row.get(0)
        })
        .optional()?;
    Ok(text.map(|t| t.and_then(|t| serde_json::from_str(&t).ok())))
}

fn new_wait(
    db: &rusqlite::Connection,
    kind: WaitKind,
    text: String,
    answerable: bool,
    doc: Option<WaitDoc>,
) -> rusqlite::Result<HumanWait> {
    db.execute("INSERT INTO wait_ids DEFAULT VALUES", [])?;
    let id = db.last_insert_rowid();
    db.execute("DELETE FROM wait_ids WHERE id = ?1", [id])?;
    Ok(HumanWait {
        id: WaitId(id as u32),
        kind,
        text,
        since: unix_now(),
        answerable,
        doc,
        instruction: None,
        images: Vec::new(),
        prompt: None,
        parts: Vec::new(),
        choices: Vec::new(),
    })
}

fn put(db: &rusqlite::Connection, id: NodeId, wait: Option<&Stored>) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE nodes SET wait = ?2 WHERE id = ?1",
        params![id.0, wait.map(to_json)],
    )?;
    Ok(())
}

fn open_ask(db: &rusqlite::Connection, wait: WaitId) -> Result<(NodeId, HumanWait), Refusal> {
    let found: Option<(u32, String)> = db
        .query_row(
            "SELECT node, wait FROM questions WHERE id = ?1",
            [wait.0],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    found
        .and_then(|(node, text)| Some((NodeId(node), serde_json::from_str(&text).ok()?)))
        .ok_or(Refusal::NoWait(wait))
}

fn questions(db: &rusqlite::Connection, id: NodeId) -> rusqlite::Result<Vec<HumanWait>> {
    let mut stmt = db.prepare_cached("SELECT wait FROM questions WHERE node = ?1 ORDER BY id")?;
    let rows = stmt
        .query_map([id.0], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .iter()
        .filter_map(|t| serde_json::from_str(t).ok())
        .collect())
}

fn done_node(db: &rusqlite::Connection, wait: WaitId) -> Result<NodeId, Refusal> {
    db.query_row("SELECT node FROM dones WHERE id = ?1", [wait.0], |row| {
        row.get(0)
    })
    .optional()?
    .map(NodeId)
    .ok_or(Refusal::NoWait(wait))
}

fn quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\\''"))
}

fn one_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    line.chars().take(TEXT_LIMIT_CHARS).collect()
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}
