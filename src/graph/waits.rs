use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use api::graph::docs::DocQuestion;
use api::graph::waits::{
    Closed, HumanWait, Prompt, ScreenWait, WaitDoc, WaitKind, WaitLimit, WaitTone,
};

use super::agent_state;
use super::closed::{self, Open};
use super::events;
use super::mailbox;
use crate::choices::{choices, offers_choices_without_tail, without_tail};
use crate::link_text::{bare_link, markdown, parts};
use crate::protocol::OpenQuestion;
use api::graph::errors::Refusal;
use api::graph::events::{AgentEvent, ClosedBy, Entry};
use api::graph::messages::{MessageKind, Question};
use api::graph::text::Block;
use api::ids::{AgentId, NodeId, WaitId};

use super::{Graph, from_json, parent_of, running_live, to_json};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WaitSource {
    Agent,
    Screen,
}

fn preview(text: String) -> String {
    if text.len() <= PREVIEW_LIMIT_BYTES {
        return text;
    }
    let cut = (0..=PREVIEW_LIMIT_BYTES)
        .rev()
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(0);
    format!("{}…", &text[..cut])
}

const PREVIEW_LIMIT_BYTES: usize = 1024;

pub(super) fn for_screen(wait: HumanWait) -> ScreenWait {
    let text = match wait.kind {
        WaitKind::Question => without_tail(&wait.text).unwrap_or(&wait.text),
        _ => &wait.text,
    };
    let blocks = text_blocks(wait.kind, text);
    screen(wait, blocks)
}

pub(super) fn for_history(wait: HumanWait) -> ScreenWait {
    let blocks = text_blocks(wait.kind, &wait.text);
    screen(wait, blocks)
}

fn text_blocks(kind: WaitKind, text: &str) -> Vec<Block> {
    match kind {
        WaitKind::Question | WaitKind::Done | WaitKind::Reply => markdown(text),
        WaitKind::Approval
        | WaitKind::Trust
        | WaitKind::Auth
        | WaitKind::Input
        | WaitKind::Stopped
        | WaitKind::Limit => Block::paragraph(parts(text)),
    }
}

fn screen(wait: HumanWait, blocks: Vec<Block>) -> ScreenWait {
    let summary = match &wait.prompt {
        Some(prompt) => prompt.summary.clone(),
        None => preview(Block::shown(&blocks)),
    };
    let tone = match wait.kind {
        WaitKind::Question
        | WaitKind::Approval
        | WaitKind::Trust
        | WaitKind::Auth
        | WaitKind::Input
        | WaitKind::Stopped
        | WaitKind::Limit => WaitTone::Ask,
        WaitKind::Done | WaitKind::Reply => WaitTone::Done,
    };
    let wait = HumanWait {
        blocks,
        choices: match wait.kind {
            WaitKind::Question => choices(&wait.text),
            WaitKind::Approval
            | WaitKind::Trust
            | WaitKind::Auth
            | WaitKind::Input
            | WaitKind::Done
            | WaitKind::Reply
            | WaitKind::Stopped
            | WaitKind::Limit => Vec::new(),
        },
        ..wait
    };
    ScreenWait {
        wait,
        tone,
        summary,
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
                                wait: {
                                    let rows = now.wait.screen.clone();
                                    shown(now.wait, prompt, rows)
                                },
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

    pub fn screen_prompt(
        &self,
        token: &str,
        prompt: Option<Prompt>,
        rows: Vec<String>,
    ) -> rusqlite::Result<()> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let Some(id) = super::nodes::caller_in(&tx, token)? else {
            return Ok(());
        };
        let mut changed = limit_rows(&tx, id, &rows)?;
        if let Some(Some(now)) = stored(&tx, id)? {
            let prompt = match now.source == WaitSource::Screen || !now.reads_screen() {
                true => now.wait.prompt.clone(),
                false => prompt,
            };
            let wait = shown(now.wait.clone(), prompt, rows);
            if wait != now.wait {
                put(
                    &tx,
                    id,
                    Some(&Stored {
                        reads_screen: Some(now.reads_screen()),
                        wait,
                        ..now
                    }),
                )?;
                changed = true;
            }
        }
        if !changed {
            return Ok(());
        }
        tx.commit()?;
        self.publish_nodes();
        Ok(())
    }

    pub fn limit_left(
        &self,
        node: NodeId,
        provider: api::profile::Provider,
        limit: Option<WaitLimit>,
        rows: Vec<String>,
    ) -> rusqlite::Result<()> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let left: bool = tx
            .query_row(
                "SELECT limited = 1 AND NOT EXISTS (SELECT 1 FROM relaunch_plans r WHERE r.node = n.id)
                 FROM nodes n WHERE n.id = ?1",
                [node.0],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(false);
        if !left || has_alert(&tx, node, WaitKind::Limit)? {
            return Ok(());
        }
        let text = format!("{} 계정이 사용 한도에 걸렸습니다.", json_name(&provider));
        let wait = HumanWait {
            limit,
            screen: rows,
            ..new_wait(&tx, WaitKind::Limit, text, false, None)?
        };
        put_alert(&tx, node, &wait)?;
        tx.commit()?;
        self.publish_nodes();
        Ok(())
    }

    pub fn screen_wait(&self, wait: WaitId) -> Result<NodeId, Refusal> {
        self.lock()
            .query_row(
                "SELECT id FROM nodes WHERE wait ->> 'id' = ?1
                 UNION ALL SELECT node FROM alerts WHERE id = ?1",
                [wait.0],
                |row| Ok(NodeId(row.get(0)?)),
            )
            .optional()?
            .ok_or(Refusal::NoWait(wait))
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
        let images = images_line(images);
        type Record = fn(api::graph::events::Who, WaitId, String, u32) -> Entry;
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

    pub fn stopped_node(&self, wait: WaitId) -> rusqlite::Result<Option<NodeId>> {
        self.lock()
            .query_row(
                "SELECT node FROM alerts WHERE id = ?1 AND wait ->> 'kind' = ?2",
                params![wait.0, json_name(&WaitKind::Stopped)],
                |row| Ok(NodeId(row.get(0)?)),
            )
            .optional()
    }

    pub fn instruct_relaunched(
        &self,
        node: NodeId,
        text: &str,
        images: &[PathBuf],
    ) -> Result<(), Refusal> {
        let text = text.trim();
        let db = self.lock();
        let body = format!(
            "New instruction from the human in the loom inbox, sent while your process was stopped: {text}{}",
            images_line(images)
        );
        mailbox::enqueue_human(&db, node, &MessageKind::Note, &body)?;
        drop(db);
        self.mail.notify_waiters();
        Ok(())
    }

    pub fn instructed(&self, node: NodeId, text: &str) -> rusqlite::Result<()> {
        let text = text.trim();
        let mut db = self.lock();
        let tx = db.transaction()?;
        let how = Closed::Instructed {
            text: text.to_owned(),
        };
        let closed = closed::close_all(&tx, Open::Done, node, &how)?;
        if closed.is_empty() {
            return Ok(());
        }
        let who = events::who(&tx, node)?;
        for wait in closed {
            let entry = Entry::Instructed {
                node: who.clone(),
                wait,
                text: text.to_owned(),
                images: 0,
            };
            events::write(&tx, &entry, None)?;
        }
        tx.commit()?;
        self.publish_nodes();
        Ok(())
    }

    pub fn doc_question(&self, doc: &str) -> Result<DocQuestion, Refusal> {
        let db = self.lock();
        let open: Option<(u32, String)> = db
            .query_row(
                "SELECT id, wait ->> '$.text' FROM questions WHERE json_valid(wait) AND wait ->> '$.doc.id' = ?1 ORDER BY id DESC LIMIT 1",
                [doc],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((wait, text)) = open {
            return Ok(DocQuestion::Open {
                wait: WaitId(wait),
                choices: choices(&text),
            });
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
                Err(Refusal::NoWait(_))
                    if tx.execute("DELETE FROM alerts WHERE id = ?1", [wait.0])? > 0 => {}
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
    put_finished(db, id, WaitKind::Done, asked, summary, images, doc)
}

pub(super) fn put_reply(
    db: &rusqlite::Connection,
    id: NodeId,
    heard: &str,
    reply: &str,
) -> rusqlite::Result<()> {
    put_finished(db, id, WaitKind::Reply, heard, reply, &[], None)
}

fn put_finished(
    db: &rusqlite::Connection,
    id: NodeId,
    kind: WaitKind,
    asked: &str,
    summary: &str,
    images: &[PathBuf],
    doc: Option<WaitDoc>,
) -> rusqlite::Result<()> {
    let summary = summary.trim().to_owned();
    let instruction: String = asked.chars().take(QUESTION_LIMIT_CHARS).collect();
    let mut wait = new_wait(db, kind, summary.clone(), true, doc.clone())?;
    wait.instruction = (!instruction.is_empty()).then(|| instruction.clone());
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

pub(super) fn open_stopped(
    db: &rusqlite::Connection,
    id: NodeId,
    text: String,
) -> rusqlite::Result<()> {
    if has_alert(db, id, WaitKind::Stopped)? {
        return Ok(());
    }
    let wait = new_wait(db, WaitKind::Stopped, text, true, None)?;
    put_alert(db, id, &wait)
}

pub(super) fn drop_alert(
    db: &rusqlite::Connection,
    id: NodeId,
    kind: WaitKind,
) -> rusqlite::Result<usize> {
    db.execute(
        "DELETE FROM alerts WHERE node = ?1 AND wait ->> 'kind' = ?2",
        params![id.0, json_name(&kind)],
    )
}

fn has_alert(db: &rusqlite::Connection, id: NodeId, kind: WaitKind) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT EXISTS (SELECT 1 FROM alerts WHERE node = ?1 AND wait ->> 'kind' = ?2)",
        params![id.0, json_name(&kind)],
        |row| row.get(0),
    )
}

fn put_alert(db: &rusqlite::Connection, id: NodeId, wait: &HumanWait) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO alerts (id, node, wait) VALUES (?1, ?2, ?3)",
        params![wait.id.0, id.0, to_json(wait)],
    )
    .map(drop)
}

fn limit_rows(db: &rusqlite::Connection, id: NodeId, rows: &[String]) -> rusqlite::Result<bool> {
    let limit = json_name(&WaitKind::Limit);
    let alerts: Vec<(u32, String)> = db
        .prepare_cached("SELECT id, wait FROM alerts WHERE node = ?1 AND wait ->> 'kind' = ?2")?
        .query_map(params![id.0, limit], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut changed = false;
    for (alert, text) in alerts {
        let Ok(wait) = serde_json::from_str::<HumanWait>(&text) else {
            continue;
        };
        if wait.screen == rows {
            continue;
        }
        let wait = HumanWait {
            screen: rows.to_vec(),
            ..wait
        };
        db.execute(
            "UPDATE alerts SET wait = ?2 WHERE id = ?1",
            params![alert, to_json(&wait)],
        )?;
        changed = true;
    }
    Ok(changed)
}

fn json_name(value: &impl Serialize) -> String {
    to_json(value).trim_matches('"').to_owned()
}

fn shown(wait: HumanWait, prompt: Option<Prompt>, rows: Vec<String>) -> HumanWait {
    let screen = match prompt {
        Some(_) => Vec::new(),
        None => rows,
    };
    HumanWait {
        prompt,
        screen,
        ..wait
    }
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
    if parent_of(db, id)?.is_some() || !running_live(db, id)? {
        return Ok(false);
    }
    let alerts: bool = db.query_row(
        "SELECT EXISTS (SELECT 1 FROM alerts WHERE node = ?1)",
        [id.0],
        |row| row.get(0),
    )?;
    Ok(stored(db, id)?.is_some_and(|wait| wait.is_none())
        && questions(db, id)?.is_empty()
        && !alerts)
}

fn stored(db: &rusqlite::Connection, id: NodeId) -> rusqlite::Result<Option<Option<Stored>>> {
    let text: Option<Option<String>> = db
        .query_row("SELECT wait FROM nodes WHERE id = ?1", [id.0], |row| {
            row.get(0)
        })
        .optional()?;
    Ok(text.map(|t| t.and_then(|t| serde_json::from_str(&t).ok())))
}

pub(super) fn new_wait(
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
        screen: Vec::new(),
        limit: None,
        blocks: Vec::new(),
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

fn images_line(images: &[PathBuf]) -> String {
    match images {
        [] => String::new(),
        paths => {
            let quoted: Vec<String> = paths.iter().map(|p| quote(&p.to_string_lossy())).collect();
            format!("\nImages: {}", quoted.join(" "))
        }
    }
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
