use rusqlite::{Connection, OptionalExtension, params};

use api::graph::docs::{Said, TalkLine, TalkTo};
use api::graph::errors::{BRIEF_LIMIT_BYTES, Refusal};
use api::graph::messages::MessageKind;
use api::ids::{NodeId, WaitId};

use super::{AgentState, Graph, ProcessOutcome, mailbox, name_of, optional_json, process_state};
use crate::protocol::MessageId;

const BY_HUMAN: &str = "human";
const BY_AGENT: &str = "agent";

const RECIPIENTS: [&str; 4] = [
    "SELECT node FROM doc_authors WHERE doc = ?1",
    "SELECT node FROM questions WHERE json_valid(wait) AND wait ->> '$.doc.id' = ?1 ORDER BY id DESC LIMIT 1",
    "SELECT node FROM dones WHERE json_valid(wait) AND wait ->> '$.doc.id' = ?1 ORDER BY id DESC LIMIT 1",
    "SELECT n.id FROM events e JOIN nodes n ON n.id = e.entry ->> '$.node.id'
     WHERE json_valid(e.entry) AND e.entry ->> 'kind' IN ('asked', 'reported') AND e.entry ->> '$.doc.id' = ?1
     ORDER BY e.id DESC LIMIT 1",
];

impl Graph {
    pub fn wrote_doc(&self, doc: &str, node: NodeId) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT INTO doc_authors (doc, node) VALUES (?1, ?2)
             ON CONFLICT (doc) DO UPDATE SET node = excluded.node",
            params![doc, node.0],
        )?;
        Ok(())
    }

    pub fn doc_recipient(&self, doc: &str) -> rusqlite::Result<Option<NodeId>> {
        let db = self.lock();
        for sql in RECIPIENTS {
            let found: Option<u32> = db.query_row(sql, [doc], |row| row.get(0)).optional()?;
            if let Some(node) = found {
                return Ok(Some(NodeId(node)));
            }
        }
        Ok(None)
    }

    pub fn talk_to(&self, node: NodeId) -> rusqlite::Result<TalkTo> {
        self.lock().query_row(
            "SELECT name, process, (SELECT state FROM agent_states WHERE node = ?1) FROM nodes WHERE id = ?1",
            [node.0],
            |row| {
                let process = optional_json::<ProcessOutcome>(row, 1)?;
                let agent = optional_json::<AgentState>(row, 2)?;
                Ok(TalkTo {
                    name: row.get(0)?,
                    process: process_state(process, agent.as_ref()),
                })
            },
        )
    }

    pub fn talk_lines(&self, doc: &str) -> rusqlite::Result<Vec<TalkLine>> {
        let db = self.lock();
        let mut stmt = db.prepare_cached(
            "SELECT t.id, t.said, t.name, t.text, t.at, t.wait, coalesce(m.delivered, 0)
             FROM doc_talk t LEFT JOIN messages m ON m.id = t.message
             WHERE t.doc = ?1 ORDER BY t.id",
        )?;
        stmt.query_map([doc], |row| {
            let said: String = row.get(1)?;
            let name: String = row.get(2)?;
            let said = match said.as_str() {
                BY_AGENT => Said::Agent { name },
                _ => Said::Human {
                    to: name,
                    answer: row.get::<_, Option<u32>>(5)?.map(WaitId),
                    received: row.get(6)?,
                },
            };
            Ok(TalkLine {
                id: row.get(0)?,
                said,
                text: row.get(3)?,
                at: row.get(4)?,
            })
        })?
        .collect()
    }

    pub fn post_talk(
        &self,
        doc: &str,
        file: &str,
        node: NodeId,
        text: &str,
    ) -> Result<(), Refusal> {
        let text = text.trim();
        if text.is_empty() {
            return Err(Refusal::EmptyAnswer);
        }
        let mut db = self.lock();
        let tx = db.transaction()?;
        let body = format!(
            "The human wrote this on the loom document page of {doc} (file: {file}):\n{text}\nIf the human asks for a change, edit that file: the open page shows each saved version at once. Reply on the page with loom answer <this message id> \"<reply>\"."
        );
        let message = mailbox::enqueue_human(&tx, node, &MessageKind::Note, &body)?;
        said_by_human(&tx, doc, node, text, message, None)?;
        tx.commit()?;
        drop(db);
        self.publish_nodes();
        self.mail.notify_waiters();
        Ok(())
    }

    pub fn answer_page(
        &self,
        from: NodeId,
        message: MessageId,
        text: &str,
    ) -> Result<Option<String>, Refusal> {
        let db = self.lock();
        let doc: Option<String> = db
            .query_row(
                "SELECT t.doc FROM doc_talk t JOIN messages m ON m.id = t.message
                 WHERE t.message = ?1 AND m.recipient = ?2",
                params![message.0, from.0],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(doc) = &doc {
            let text = text.trim();
            if text.is_empty() {
                return Err(Refusal::EmptyAnswer);
            }
            if text.len() > BRIEF_LIMIT_BYTES {
                return Err(Refusal::BriefTooLong(text.len()));
            }
            db.execute(
                "INSERT INTO doc_talk (doc, said, name, text) VALUES (?1, ?2, ?3, ?4)",
                params![doc, BY_AGENT, name_of(&db, from)?, text],
            )?;
        }
        Ok(doc)
    }
}

pub(super) fn said_by_human(
    db: &Connection,
    doc: &str,
    to: NodeId,
    text: &str,
    message: MessageId,
    answer: Option<WaitId>,
) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO doc_talk (doc, said, name, text, message, wait) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![doc, BY_HUMAN, name_of(db, to)?, text, message.0, answer.map(|w| w.0)],
    )?;
    Ok(())
}
