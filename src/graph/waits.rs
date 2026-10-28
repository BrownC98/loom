use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::agent_state::{self, AgentEvent};
use super::closed::{self, Closed, Open};
use super::events::{self, ClosedBy, Entry};
use super::mailbox;
use crate::choices::{Choice, choices, offers_choices_without_tail};
use crate::link_text::{TextPart, bare_link, parts};
use crate::protocol::{
    AgentId, MessageKind, OpenQuestion, Prompt, Question, Refusal, WaitDoc, WaitId, WaitKind,
};

use super::{Graph, NodeId, NodeState, from_json, parent_of, state_of, to_json};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WaitSource {
    Agent,
    Screen,
}

/// DB에 두는 대기 한 건.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
pub struct HumanWait {
    pub id: WaitId,
    pub kind: WaitKind,
    /// `loom ask` 질문과 완료는 전문, 다른 대기는 한 줄이다. 없으면 빈 글이고, 화면은 종류 이름만 보인다.
    pub text: String,
    /// 대기가 시작된 유닉스 초.
    #[ts(type = "number")]
    pub since: i64,
    /// `loom ask` 질문이나 완료다. 화면에서 답할 수 있다(`Graph::answer`). 완료의 답은 이어서 맡기는 일이다. 다른 대기는 `prompt`의 버튼·입력칸이나 그 노드 터미널에서 답한다.
    pub answerable: bool,
    pub doc: Option<WaitDoc>,
    /// 완료(`Done`)만: 그 일을 맡긴 사람의 지시(`loom done --asked`). `text`는 결과(`loom done`의 요약)다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub instruction: Option<String>,
    /// 완료(`Done`)만: 사람이 그 지시에 준 그림의 데몬 기계 경로(`loom done --image`). 화면은 `GET /api/waits/{id}/images/{index}`로 읽는다.
    #[serde(default)]
    pub images: Vec<String>,
    /// 신호(훅·서버·화면)의 대기만: 지금 터미널 화면에서 읽은 창이나 서버가 준 창. 읽지 못했으면 없다. 대기와 함께 DB에 둔다(`Graph::set_wait`).
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

/// 문서 페이지(`/docs/{id}`)의 응답 막대 상태: 그 문서가 붙은 가장 최근 질문. 목업 docs/mockups/doc-answer.html.
#[derive(Serialize, TS, PartialEq, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum DocQuestion {
    None,
    Open {
        wait: WaitId,
    },
    Answered {
        wait: WaitId,
        text: String,
        /// 답한 유닉스 초.
        #[ts(type = "number")]
        at: i64,
    },
    Closed {
        wait: WaitId,
    },
}

/// 화면에 보내는 대기: 저장한 대기와 데몬이 정한 표시 값.
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct ScreenWait {
    #[serde(flatten)]
    #[ts(flatten)]
    pub wait: HumanWait,
    pub tone: WaitTone,
    /// 한 줄 미리 보기: 창이 있으면 `prompt.summary`, 없으면 `parts`의 글.
    pub summary: String,
}

/// 대기의 색과 아이콘, 알림 채널.
#[derive(Clone, Copy, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum WaitTone {
    Ask,
    Done,
}

impl HumanWait {
    pub(super) fn for_screen(self) -> ScreenWait {
        let parts = parts(&self.text);
        let summary = match &self.prompt {
            Some(prompt) => prompt.summary.clone(),
            None => parts
                .iter()
                .map(|part| match part {
                    TextPart::Text { text } => text.as_str(),
                    TextPart::Link { label, .. } | TextPart::Doc { label } => label.as_str(),
                })
                .collect(),
        };
        let tone = match self.kind {
            WaitKind::Question
            | WaitKind::Approval
            | WaitKind::Trust
            | WaitKind::Auth
            | WaitKind::Input => WaitTone::Ask,
            WaitKind::Done => WaitTone::Done,
        };
        let wait = HumanWait {
            parts,
            choices: match self.kind {
                WaitKind::Question => choices(&self.text),
                WaitKind::Approval
                | WaitKind::Trust
                | WaitKind::Auth
                | WaitKind::Input
                | WaitKind::Done => Vec::new(),
            },
            ..self
        };
        ScreenWait {
            wait,
            tone,
            summary,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct Stored {
    #[serde(flatten)]
    pub wait: HumanWait,
    pub source: WaitSource,
    #[serde(default)]
    reads_screen: Option<bool>,
}

impl Stored {
    fn reads_screen(&self) -> bool {
        self.reads_screen.unwrap_or(self.wait.prompt.is_none())
    }
}

const TEXT_LIMIT_CHARS: usize = 200;
const QUESTION_LIMIT_CHARS: usize = 1000;
pub const HUMAN: &str = "human";

impl Graph {
    pub fn set_wait(
        &self,
        token: &str,
        source: WaitSource,
        wait: Option<(WaitKind, &str, Option<Prompt>)>,
    ) -> rusqlite::Result<()> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let Some(id) = super::nodes::caller_in(&tx, token)? else {
            return Ok(());
        };
        let Some(now) = stored(&tx, id)? else {
            return Ok(());
        };
        let next = match wait {
            Some((kind, text, prompt)) => {
                let text = one_line(text);
                match now {
                    Some(now) if now.wait.kind == kind && now.wait.text == text => {
                        if now.wait.prompt == prompt || (prompt.is_none() && source != now.source) {
                            return Ok(());
                        }
                        put(
                            &tx,
                            id,
                            Some(&Stored {
                                reads_screen: Some(now.reads_screen()),
                                wait: HumanWait { prompt, ..now.wait },
                                ..now
                            }),
                        )?;
                        tx.commit()?;
                        self.publish_nodes();
                        return Ok(());
                    }
                    Some(_) | None => {}
                }
                Some(Stored {
                    reads_screen: Some(source == WaitSource::Screen || prompt.is_none()),
                    wait: HumanWait {
                        prompt,
                        ..new_wait(&tx, kind, text, false, None)?
                    },
                    source,
                })
            }
            None if now.as_ref().is_some_and(|n| n.source == source) => None,
            None => return Ok(()),
        };
        put(&tx, id, next.as_ref())?;
        let event = match next {
            Some(_) => AgentEvent::Dialog,
            None => AgentEvent::DialogCleared,
        };
        agent_state::apply(&tx, id, event)?;
        tx.commit()?;
        self.publish_nodes();
        Ok(())
    }

    pub fn screen_prompt(&self, token: &str, prompt: Option<Prompt>) -> rusqlite::Result<()> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let Some(id) = super::nodes::caller_in(&tx, token)? else {
            return Ok(());
        };
        let Some(Some(now)) = stored(&tx, id)? else {
            return Ok(());
        };
        if now.source == WaitSource::Screen || !now.reads_screen() || now.wait.prompt == prompt {
            return Ok(());
        }
        put(
            &tx,
            id,
            Some(&Stored {
                reads_screen: Some(true),
                wait: HumanWait { prompt, ..now.wait },
                ..now
            }),
        )?;
        tx.commit()?;
        self.publish_nodes();
        Ok(())
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
        self.publish_nodes();
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
        let text = text.to_owned();
        let (open, how) = if matches!(what, MessageKind::Answer { .. }) {
            tx.execute("INSERT OR IGNORE INTO answers (id) VALUES (?1)", [wait.0])?;
            (Open::Question, Closed::Answered { text: text.clone() })
        } else {
            (Open::Done, Closed::Instructed { text: text.clone() })
        };
        mailbox::enqueue_human(&tx, node, &what, &body)?;
        closed::close(&tx, open, wait, &how)?;
        let entry = record(events::who(&tx, node)?, wait, text, count);
        events::write(&tx, &entry, None)?;
        tx.commit()?;
        self.publish_nodes();
        self.mail.notify_waiters();
        Ok(node)
    }

    pub fn doc_question(&self, doc: &str) -> Result<DocQuestion, Refusal> {
        let db = self.lock();
        let open: Option<u32> = db
            .query_row(
                "SELECT id FROM questions WHERE json_valid(wait) AND wait ->> '$.doc.id' = ?1 ORDER BY id DESC LIMIT 1",
                [doc],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(wait) = open {
            return Ok(DocQuestion::Open { wait: WaitId(wait) });
        }
        let asked: Option<u32> = db
            .query_row(
                "SELECT entry ->> '$.wait' FROM events WHERE json_valid(entry) AND entry ->> '$.kind' = 'asked' AND entry ->> '$.doc.id' = ?1 ORDER BY id DESC LIMIT 1",
                [doc],
                |row| row.get(0),
            )
            .optional()?;
        let Some(wait) = asked else {
            return Ok(DocQuestion::None);
        };
        let answer: Option<(String, i64)> = db
            .query_row(
                "SELECT entry ->> '$.text', at FROM events WHERE json_valid(entry) AND entry ->> '$.kind' = 'answered' AND entry ->> '$.wait' = ?1 ORDER BY id DESC LIMIT 1",
                [wait],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let wait = WaitId(wait);
        Ok(match answer {
            Some((text, at)) => DocQuestion::Answered { wait, text, at },
            None => DocQuestion::Closed { wait },
        })
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
            closed::close(
                &tx,
                Open::Question,
                *q,
                &Closed::Unanswered {
                    by: ClosedBy::Agent,
                },
            )?;
            unanswered(&tx, id, *q, ClosedBy::Agent)?;
        }
        tx.commit()?;
        if !closing.is_empty() {
            self.publish_nodes();
        }
        Ok(closing)
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

    pub fn drafted(&self, id: NodeId) -> rusqlite::Result<bool> {
        self.lock()
            .query_row(
                "SELECT drafted = 1 FROM nodes WHERE id = ?1",
                [id.0],
                |row| row.get(0),
            )
            .optional()
            .map(|drafted| drafted.unwrap_or(false))
    }

    pub fn begin_draft(&self, id: NodeId) -> rusqlite::Result<()> {
        if !self.drafted(id)? {
            self.lock().execute(
                "UPDATE nodes SET drafted = 1 WHERE id = ?1 AND drafted = 0",
                [id.0],
            )?;
        }
        Ok(())
    }

    pub fn end_draft(&self, id: NodeId) -> rusqlite::Result<bool> {
        if !self.drafted(id)? {
            return Ok(false);
        }
        Ok(self.lock().execute(
            "UPDATE nodes SET drafted = 0 WHERE id = ?1 AND drafted = 1",
            [id.0],
        )? > 0)
    }

    pub fn typed(&self, id: NodeId) -> rusqlite::Result<bool> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let changed = tx.execute(
            "UPDATE nodes SET wait = NULL WHERE id = ?1 AND wait IS NOT NULL",
            [id.0],
        )?;
        if changed == 0 {
            return Ok(false);
        }
        agent_state::apply(&tx, id, AgentEvent::DialogCleared)?;
        tx.commit()?;
        self.publish_nodes();
        Ok(true)
    }

    pub fn dismiss(&self, wait: WaitId) -> Result<(), Refusal> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let node = closed::close(
            &tx,
            Open::Question,
            wait,
            &Closed::Unanswered {
                by: ClosedBy::Human,
            },
        )?;
        let asked = node.is_some();
        match node {
            Some(node) => {
                mailbox::enqueue_human(
                    &tx,
                    node,
                    &MessageKind::Note,
                    &format!("{wait} was closed without an answer."),
                )?;
                unanswered(&tx, node, wait, ClosedBy::Human)?;
            }
            None => match done_node(&tx, wait) {
                Ok(node) => {
                    closed::close(&tx, Open::Done, wait, &Closed::Confirmed)?;
                    let entry = Entry::Confirmed {
                        node: events::who(&tx, node)?,
                        wait,
                    };
                    events::write(&tx, &entry, None)?;
                }
                Err(Refusal::NoWait(_)) => {
                    let node = tx
                        .query_row(
                            "UPDATE nodes SET wait = NULL WHERE wait ->> 'id' = ?1 RETURNING id",
                            [wait.0],
                            |row| row.get(0),
                        )
                        .optional()?
                        .map(NodeId)
                        .ok_or(Refusal::NoWait(wait))?;
                    agent_state::apply(&tx, node, AgentEvent::DialogCleared)?;
                }
                Err(e) => return Err(e),
            },
        }
        tx.commit()?;
        if asked {
            self.mail.notify_waiters();
        }
        self.publish_nodes();
        Ok(())
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

    pub fn dialog(&self, wait: WaitId) -> Result<(NodeId, Option<AgentId>), Refusal> {
        let found: Option<(u32, Option<AgentId>, Stored)> = self
            .lock()
            .query_row(
                "SELECT id, agent, wait FROM nodes WHERE wait ->> 'id' = ?1",
                [wait.0],
                |row| Ok((row.get(0)?, from_json(row, 1)?, from_json(row, 2)?)),
            )
            .optional()?;
        match found {
            Some((node, agent, _)) => Ok((NodeId(node), agent)),
            _ => Err(Refusal::NoWait(wait)),
        }
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
    let how = Closed::Unanswered { by: ClosedBy::Exit };
    for wait in closed::close_all(db, Open::Question, id, &how)? {
        unanswered(db, id, wait, ClosedBy::Exit)?;
    }
    Ok(())
}

pub(super) fn drop_on_removal(db: &rusqlite::Connection, id: NodeId) -> rusqlite::Result<()> {
    closed::close_all(db, Open::Question, id, &Closed::NodeRemoved)?;
    closed::close_all(db, Open::Done, id, &Closed::NodeRemoved)?;
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

pub(super) fn unreported_root(db: &rusqlite::Connection, id: NodeId) -> rusqlite::Result<bool> {
    if parent_of(db, id)?.is_some() || state_of(db, id)? != NodeState::Running {
        return Ok(false);
    }
    Ok(stored(db, id)?.is_some_and(|wait| wait.is_none()) && questions(db, id)?.is_empty())
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
