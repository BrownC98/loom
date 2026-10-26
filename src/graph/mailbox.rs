use std::path::PathBuf;
use std::pin::pin;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::events::{self, Entry, GateTrace, Step, Via};
use super::waits;
use super::{
    Graph, HUMAN, NodeId, NodeState, REACHABLE, Toward, ambiguous, from_json, optional_json,
    parent_of, set_state, state_of, to_json,
};
use crate::choices::offers_choices_without_tail;
use crate::link_text::bare_link;
use crate::protocol::{
    AgentId, BATCH_SEPARATOR, BRIEF_LIMIT_BYTES, Delivery, Kind, LOOM, MAILBOX_LIMIT, MessageId,
    MessageKind, Question, Refusal, SUMMARY_LIMIT_BYTES, WaitDoc,
};

fn wakes(delivery: &Delivery) -> bool {
    delivery.kind != Kind::Queue || delivery.what.as_ref().is_none_or(MessageKind::wakes)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Sent {
    pub id: MessageId,
    pub recipient: Option<NodeId>,
}

struct Outgoing<'a> {
    kind: Kind,
    what: MessageKind,
    body: &'a str,
    reply_to: Option<MessageId>,
    via: Option<&'a Via>,
}

#[derive(Default)]
pub(super) struct Stamp {
    pub via: Option<Via>,
    pub gate: Option<GateTrace>,
}

impl Graph {
    pub fn related(&self, from: NodeId, to: &str) -> Result<(NodeId, Option<AgentId>), Refusal> {
        related(&self.lock(), from, to)
    }

    pub fn any_node(&self, from: NodeId, to: &str) -> Result<NodeId, Refusal> {
        let db = self.lock();
        if let Some(id) = node_id(&db, from, to)? {
            let exists: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM nodes WHERE id = ?1)",
                [id],
                |r| r.get(0),
            )?;
            return if exists {
                Ok(NodeId(id))
            } else {
                Err(Refusal::NotFound(to.to_owned()))
            };
        }
        match related(&db, from, to) {
            Err(Refusal::NotRelated(_)) => {}
            found => return found.map(|(id, _)| id),
        }
        drop(db);
        self.find(to)
    }

    pub fn steerable(&self, from: NodeId, to: &str) -> Result<(NodeId, Option<AgentId>), Refusal> {
        steerable(&self.lock(), from, to)
    }

    pub fn note(
        &self,
        from: NodeId,
        to: &str,
        kind: Kind,
        body: &str,
        via: Option<&Via>,
    ) -> Result<Sent, Refusal> {
        self.post(
            from,
            to,
            Outgoing {
                kind,
                what: MessageKind::Note,
                body,
                reply_to: None,
                via,
            },
            |_| {},
        )
    }

    pub fn interrupt(
        &self,
        from: NodeId,
        to: &str,
        body: &str,
        via: Option<&Via>,
        stop: impl FnOnce(NodeId),
    ) -> Result<Sent, Refusal> {
        self.post(
            from,
            to,
            Outgoing {
                kind: Kind::Interrupt,
                what: MessageKind::Note,
                body,
                reply_to: None,
                via,
            },
            stop,
        )
    }

    pub fn ask_parent(
        &self,
        from: NodeId,
        question: &str,
        via: Option<&Via>,
    ) -> Result<Sent, Refusal> {
        let question = question.trim();
        if question.is_empty() {
            return Err(Refusal::EmptyQuestion);
        }
        if offers_choices_without_tail(question) {
            return Err(Refusal::BadChoices);
        }
        self.post(
            from,
            PARENT,
            Outgoing {
                kind: Kind::Queue,
                what: MessageKind::Ask,
                body: question,
                reply_to: None,
                via,
            },
            |_| {},
        )
    }

    pub fn answer_ask(
        &self,
        from: NodeId,
        request: MessageId,
        body: &str,
        via: Option<&Via>,
    ) -> Result<Sent, Refusal> {
        let sender: Option<Option<u32>> = self
            .lock()
            .query_row(
                "SELECT sender FROM messages WHERE id = ?1 AND recipient = ?2 AND what ->> 'kind' = 'ask'",
                params![request.0, from.0],
                |row| row.get(0),
            )
            .optional()?;
        let Some(Some(sender)) = sender else {
            return Err(Refusal::NotAsk(request.0));
        };
        self.post(
            from,
            &format!("#{sender}"),
            Outgoing {
                kind: Kind::Queue,
                what: MessageKind::Answer {
                    question: Question::Message { id: request.0 },
                },
                body,
                reply_to: Some(request),
                via,
            },
            |_| {},
        )
    }

    fn post(
        &self,
        from: NodeId,
        to: &str,
        message: Outgoing<'_>,
        before_delivery: impl FnOnce(NodeId),
    ) -> Result<Sent, Refusal> {
        let Outgoing {
            kind,
            what,
            body,
            reply_to,
            via,
        } = message;
        if body.len() > BRIEF_LIMIT_BYTES {
            return Err(Refusal::BriefTooLong(body.len()));
        }
        let mut db = self.lock();
        let found = match kind {
            Kind::Queue => address(&db, from, to),
            Kind::Steer => steerable(&db, from, to).map(|(id, _)| id),
            Kind::Interrupt => related(&db, from, to).map(|(id, _)| id),
        };
        let to_id = match found {
            Ok(to_id) => Some(to_id),
            Err(Refusal::NotFound(_))
                if kind == Kind::Queue
                    && reply_to.is_none()
                    && (to == PARENT || node_id(&db, from, to)?.is_none())
                    && parent_of(&db, from)?.is_none() =>
            {
                None
            }
            Err(e) => return Err(e),
        };
        if let Some(to_id) = to_id {
            if unreachable(&db, to_id)? {
                return Err(Refusal::Ended(to.to_owned()));
            }
            check_room(&db, to_id, to)?;
        } else {
            check_held_room(&db, from)?;
        }
        let tx = db.transaction()?;
        let stamp = Stamp {
            via: via.cloned(),
            gate: None,
        };
        let id = enqueue(&tx, from, to_id, kind, &what, body, stamp)?;
        if let Some(request) = reply_to {
            let valid: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE id = ?1 AND recipient = ?2 AND sender = ?3)",
                params![request.0, from.0, to_id.map(|id| id.0)], |row| row.get(0),
            )?;
            if !valid {
                return Err(Refusal::NotFound(format!("메시지 #{}", request.0)));
            }
            tx.execute(
                "UPDATE messages SET reply_to = ?2 WHERE id = ?1",
                params![id.0, request.0],
            )?;
        }
        if let Some(to_id) = to_id {
            before_delivery(to_id);
        }
        tx.commit()?;
        match to_id {
            Some(to_id) => self.publish_sent(&db, from, to_id)?,
            None => self.publish_node(&db, from)?,
        }
        Ok(Sent {
            id,
            recipient: to_id,
        })
    }

    pub(super) fn publish_sent(
        &self,
        db: &Connection,
        from: NodeId,
        to: NodeId,
    ) -> rusqlite::Result<()> {
        self.publish_node(db, to)?;
        if parent_of(db, from)? == Some(to) {
            self.publish_message(from, Toward::Parent);
        } else if parent_of(db, to)? == Some(from) {
            self.publish_message(to, Toward::Child);
        } else {
            self.publish_hop(from, to);
        }
        Ok(())
    }

    pub async fn wait(
        &self,
        node: NodeId,
        reply_to: Option<MessageId>,
    ) -> Result<Delivery, Refusal> {
        let Some(request) = reply_to else {
            let mut taken = self.next(node, Kind::ALL, 0).await?;
            return Ok(taken.remove(0));
        };
        let owns: bool = self.lock().query_row(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE id = ?1 AND sender = ?2)",
            params![request.0, node.0],
            |row| row.get(0),
        )?;
        if !owns {
            return Err(Refusal::NotFound(format!("메시지 #{}", request.0)));
        }
        loop {
            let mut notified = pin!(self.mail_changed());
            notified.as_mut().enable();
            let delivery = self
                .lock()
                .query_row(
                    "SELECT id, sender_name, kind, body, what FROM messages
                     WHERE recipient = ?1 AND reply_to = ?2 ORDER BY id LIMIT 1",
                    params![node.0, request.0],
                    read_delivery,
                )
                .optional()?;
            if let Some(delivery) = delivery {
                self.lock().execute(
                    "UPDATE messages SET sent_at = unixepoch()
                     WHERE id = ?1 AND delivered = 0 AND sent_at IS NULL",
                    [delivery.id.0],
                )?;
                return Ok(delivery);
            }
            notified.await;
        }
    }

    pub async fn next(
        &self,
        node: NodeId,
        kinds: &[Kind],
        limit_bytes: usize,
    ) -> Result<Vec<Delivery>, Refusal> {
        loop {
            let mut notified = pin!(self.mail_changed());
            notified.as_mut().enable();
            let taken = self.take(node, kinds, limit_bytes)?;
            if !taken.is_empty() {
                return Ok(taken);
            }
            notified.await;
        }
    }

    pub fn interrupt_pending(&self, node: NodeId) -> rusqlite::Result<bool> {
        self.lock().query_row(
            "SELECT EXISTS(SELECT 1 FROM messages
             WHERE recipient = ?1 AND delivered = 0 AND printed = 0 AND kind = ?2)",
            params![node.0, to_json(&Kind::Interrupt)],
            |row| row.get(0),
        )
    }

    pub fn take(
        &self,
        node: NodeId,
        kinds: &[Kind],
        limit_bytes: usize,
    ) -> Result<Vec<Delivery>, Refusal> {
        self.take_if(node, kinds, limit_bytes, |_| true)
    }

    pub fn take_waking(
        &self,
        node: NodeId,
        kinds: &[Kind],
        limit_bytes: usize,
    ) -> Result<Vec<Delivery>, Refusal> {
        self.take_if(node, kinds, limit_bytes, |queued| queued.iter().any(wakes))
    }

    fn take_if(
        &self,
        node: NodeId,
        kinds: &[Kind],
        limit_bytes: usize,
        wanted: impl FnOnce(&[Delivery]) -> bool,
    ) -> Result<Vec<Delivery>, Refusal> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let kinds = serde_json::to_string(&kinds.iter().map(to_json).collect::<Vec<_>>())
            .expect("문자열 목록은 늘 직렬화된다");
        let queued = tx
            .prepare(
                "SELECT id, sender_name, kind, body, what FROM messages
                 WHERE recipient = ?1 AND delivered = 0 AND sent_at IS NULL
                 AND kind IN (SELECT value FROM json_each(?2)) ORDER BY id",
            )?
            .query_map(params![node.0, kinds], read_delivery)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if !wanted(&queued) {
            return Ok(Vec::new());
        }
        let mut taken = Vec::new();
        let mut total = 0;
        for delivery in queued {
            total += delivery.text().len() + BATCH_SEPARATOR.len();
            if !taken.is_empty() && total > limit_bytes {
                break;
            }
            tx.execute(
                "UPDATE messages SET sent_at = unixepoch() WHERE id = ?1",
                [delivery.id.0],
            )?;
            taken.push(delivery);
        }
        let resumed = !taken.is_empty() && resume(&tx, node)?;
        tx.commit()?;
        if resumed {
            self.publish_node(&db, node)?;
        }
        Ok(taken)
    }

    pub fn sleepers_with_mail(&self) -> rusqlite::Result<Vec<NodeId>> {
        let db = self.lock();
        let sleepers = db
            .prepare(
                "SELECT DISTINCT n.id FROM nodes n JOIN messages m ON m.recipient = n.id
                 WHERE n.asleep = 1 AND m.delivered = 0 ORDER BY n.id",
            )?
            .query_map([], |row| Ok(NodeId(row.get(0)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut waking = Vec::new();
        for node in sleepers {
            if queued(&db, node)? > 0 {
                waking.push(node);
            }
        }
        Ok(waking)
    }

    pub fn human_input(&self, node: NodeId) -> Result<(), Refusal> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let steered = steer_where(&tx, node, |d| !wakes(d))?;
        let resumed = resume(&tx, node)?;
        tx.commit()?;
        if resumed || steered > 0 {
            self.publish_node(&db, node)?;
        }
        if steered > 0 {
            self.mail.notify_waiters();
        }
        Ok(())
    }

    pub fn steer_queued(&self, node: NodeId) -> Result<(), Refusal> {
        let db = self.lock();
        let agent: Option<AgentId> = db
            .query_row("SELECT agent FROM nodes WHERE id = ?1", [node.0], |row| {
                from_json(row, 0)
            })
            .optional()?
            .flatten();
        if !agent.as_ref().is_some_and(self.linked) {
            let notices: Vec<MessageId> = db
                .prepare(
                    "SELECT id, sender_name, kind, body, what FROM messages
                     WHERE recipient = ?1 AND delivered = 0",
                )?
                .query_map([node.0], read_delivery)?
                .collect::<rusqlite::Result<Vec<_>>>()?
                .into_iter()
                .filter(|d| d.what.as_ref().is_some_and(|w| !w.wakes()))
                .map(|d| d.id)
                .collect();
            drop(db);
            if notices.is_empty() {
                return Ok(());
            }
            return self.ack(node, &notices);
        }
        steer_where(&db, node, |_| true)?;
        drop(db);
        self.mail.notify_waiters();
        Ok(())
    }

    pub fn ack(&self, node: NodeId, ids: &[MessageId]) -> Result<(), Refusal> {
        let mut db = self.lock();
        let ids = serde_json::to_string(&ids.iter().map(|id| id.0).collect::<Vec<_>>())
            .expect("숫자 목록은 늘 직렬화된다");
        let agent: Option<AgentId> = db
            .query_row("SELECT agent FROM nodes WHERE id = ?1", [node.0], |row| {
                from_json(row, 0)
            })
            .optional()?
            .flatten();
        let linked = agent.as_ref().is_some_and(self.linked);
        let tx = db.transaction()?;
        let handed = match linked {
            true => "AND sent_at IS NOT NULL",
            false => "",
        };
        tx.execute(
            &format!(
                "UPDATE messages SET printed = 1
                 WHERE recipient = ?1 AND delivered = 0 {handed}
                 AND id IN (SELECT value FROM json_each(?2))"
            ),
            params![node.0, ids],
        )?;
        let read = match linked {
            true => 0,
            false => read(&tx, node)?,
        };
        tx.commit()?;
        if read > 0 {
            self.publish_node(&db, node)?;
        }
        Ok(())
    }

    pub fn release(&self, node: NodeId, ids: &[MessageId]) -> Result<(), Refusal> {
        let db = self.lock();
        let ids = serde_json::to_string(&ids.iter().map(|id| id.0).collect::<Vec<_>>())
            .expect("숫자 목록은 늘 직렬화된다");
        let released = db.execute(
            "UPDATE messages SET sent_at = NULL, printed = 0
             WHERE recipient = ?1 AND delivered = 0 AND id IN (SELECT value FROM json_each(?2))",
            params![node.0, ids],
        )?;
        if released > 0 {
            self.publish_node(&db, node)?;
        }
        Ok(())
    }

    pub fn done(
        &self,
        node: NodeId,
        report: &Report,
        asked: Option<&str>,
        images: &[PathBuf],
        doc: Option<WaitDoc>,
        via: Option<&Via>,
    ) -> Result<(), Refusal> {
        let summary = &with_kept(report.summary, report.keep);
        if summary.len() > SUMMARY_LIMIT_BYTES {
            return Err(Refusal::SummaryTooLong(summary.len()));
        }
        let bare = bare_link(summary);
        let mut db = self.lock();
        let parent = parent_of(&db, node)?;
        super::nodes::check_spent(&db, node, report.keep)?;
        let asked = asked.map(str::trim).filter(|a| !a.is_empty());
        if parent.is_none() && asked.is_none() {
            return Err(Refusal::NoAsked);
        }
        if parent.is_none()
            && let Some(found) = bare
        {
            return Err(Refusal::BareLink(found));
        }
        if parent.is_some() && doc.is_some() {
            return Err(Refusal::DocHasParent);
        }
        let tx = db.transaction()?;
        set_state(
            &tx,
            node,
            &NodeState::Done {
                summary: summary.to_owned(),
            },
        )?;
        match asked {
            Some(asked) if parent.is_none() => {
                waits::put_done(&tx, node, asked, summary, images, doc)?
            }
            _ => {
                let stamp = Stamp {
                    via: via.cloned(),
                    gate: None,
                };
                let what = MessageKind::Done {
                    file: report.file.map(str::to_owned),
                    warning: report.warning.clone(),
                };
                enqueue(&tx, node, parent, Kind::Queue, &what, summary, stamp)?;
            }
        }
        tx.commit()?;
        self.publish_node(&db, node)?;
        if let Some(parent) = parent {
            self.publish_node(&db, parent)?;
            self.publish_message(node, Toward::Parent);
        }
        Ok(())
    }
}

impl Graph {
    pub fn human_typed(&self, node: NodeId, text: &str) -> Result<(), Refusal> {
        let mut db = self.lock();
        let Some(parent) = parent_of(&db, node)? else {
            return Ok(());
        };
        if text.trim().is_empty() {
            return Ok(());
        }
        if unreachable(&db, parent)? {
            return Ok(());
        }
        let tx = db.transaction()?;
        let what = MessageKind::HumanInput {
            node: super::name_of(&tx, node)?,
        };
        enqueue(
            &tx,
            node,
            Some(parent),
            Kind::Queue,
            &what,
            text,
            Stamp::default(),
        )?;
        tx.commit()?;
        self.publish_sent(&db, node, parent)?;
        Ok(())
    }
}

fn unreachable(db: &Connection, node: NodeId) -> rusqlite::Result<bool> {
    db.query_row(
        &format!("SELECT NOT {REACHABLE} FROM nodes WHERE id = ?1"),
        [node.0],
        |row| row.get(0),
    )
}

pub struct Report<'a> {
    pub summary: &'a str,
    pub file: Option<&'a str>,
    pub warning: Option<String>,
    pub keep: &'a [Kept],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Kept {
    pub name: String,
    pub reason: String,
}

fn with_kept(summary: &str, keep: &[Kept]) -> String {
    if keep.is_empty() {
        return summary.to_owned();
    }
    let kept: Vec<String> = keep
        .iter()
        .map(|k| format!("{} ({})", k.name, k.reason))
        .collect();
    format!("{summary}\nKept: {}", kept.join("; "))
}

pub(super) fn read(db: &Connection, node: NodeId) -> rusqlite::Result<usize> {
    let read = db
        .prepare(
            "UPDATE messages SET delivered = 1
             WHERE recipient = ?1 AND delivered = 0 AND printed = 1
             RETURNING id, kind, sender, sender_name, what",
        )?
        .query_map([node.0], |row| {
            Ok((
                MessageId(row.get(0)?),
                from_json::<Kind>(row, 1)?,
                row.get::<_, Option<u32>>(2)?.map(NodeId),
                row.get::<_, String>(3)?,
                optional_json::<MessageKind>(row, 4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if read.is_empty() {
        return Ok(0);
    }
    let to = events::who(db, node)?;
    for (id, how, sender, name, what) in read.iter().cloned() {
        let entry = Entry::Message {
            how,
            what,
            from: events::Who { id: sender, name },
            to: Some(to.clone()),
            step: Step::Delivered,
            via: None,
            gate: None,
        };
        events::write(db, &entry, Some(id))?;
    }
    Ok(read.len())
}

pub(super) fn requeue_unprinted(db: &Connection, node: NodeId) -> rusqlite::Result<usize> {
    db.execute(
        "UPDATE messages SET sent_at = NULL
         WHERE recipient = ?1 AND delivered = 0 AND printed = 0 AND sent_at IS NOT NULL",
        [node.0],
    )
}

pub(super) fn requeue_handed(db: &Connection) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE messages SET sent_at = NULL
         WHERE delivered = 0 AND printed = 0 AND sent_at IS NOT NULL",
        [],
    )?;
    Ok(())
}

pub(super) fn requeue_unread(db: &Connection, node: NodeId) -> rusqlite::Result<usize> {
    db.execute(
        "UPDATE messages SET sent_at = NULL, printed = 0
         WHERE recipient = ?1 AND delivered = 0 AND sent_at IS NOT NULL",
        [node.0],
    )
}

fn resume(db: &Connection, node: NodeId) -> rusqlite::Result<bool> {
    let done = matches!(state_of(db, node), Ok(NodeState::Done { .. }));
    if done {
        set_state(db, node, &NodeState::Running)?;
    }
    Ok(done)
}

fn steer_where(
    db: &Connection,
    node: NodeId,
    wanted: impl Fn(&Delivery) -> bool,
) -> rusqlite::Result<usize> {
    let queued = db
        .prepare(
            "SELECT id, sender_name, kind, body, what FROM messages
             WHERE recipient = ?1 AND delivered = 0 AND sent_at IS NULL AND kind = ?2",
        )?
        .query_map(params![node.0, to_json(&Kind::Queue)], read_delivery)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut steered = 0;
    for delivery in queued.iter().filter(|d| wanted(d)) {
        db.execute(
            "UPDATE messages SET kind = ?2 WHERE id = ?1",
            params![delivery.id.0, to_json(&Kind::Steer)],
        )?;
        steered += 1;
    }
    Ok(steered)
}

fn queued(db: &Connection, to: NodeId) -> rusqlite::Result<u32> {
    Ok(waking(db, to)?.len() as u32)
}

pub(super) fn newest(db: &Connection, to: NodeId) -> rusqlite::Result<Option<u32>> {
    Ok(waking(db, to)?.iter().map(|d| d.id.0).max())
}

fn waking(db: &Connection, to: NodeId) -> rusqlite::Result<Vec<Delivery>> {
    let queued = db
        .prepare(
            "SELECT id, sender_name, kind, body, what FROM messages
             WHERE recipient = ?1 AND delivered = 0",
        )?
        .query_map([to.0], read_delivery)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(queued.into_iter().filter(wakes).collect())
}

fn check_room(db: &Connection, to: NodeId, name: &str) -> Result<(), Refusal> {
    if queued(db, to)? >= MAILBOX_LIMIT {
        return Err(Refusal::MailboxFull(name.to_owned()));
    }
    Ok(())
}

fn check_held_room(db: &Connection, from: NodeId) -> Result<(), Refusal> {
    let held: u32 = db.query_row(
        "SELECT count(*) FROM messages WHERE recipient IS NULL AND sender = ?1",
        [from.0],
        |row| row.get(0),
    )?;
    if held >= MAILBOX_LIMIT {
        return Err(Refusal::HeldFull);
    }
    Ok(())
}

pub(super) fn enqueue(
    db: &Connection,
    from: NodeId,
    to: Option<NodeId>,
    kind: Kind,
    what: &MessageKind,
    body: &str,
    stamp: Stamp,
) -> rusqlite::Result<MessageId> {
    db.execute(
        "INSERT INTO messages (recipient, sender, sender_name, kind, body, what)
         VALUES (?1, ?2, (SELECT name FROM nodes WHERE id = ?2), ?3, ?4, ?5)",
        params![
            to.map(|to| to.0),
            from.0,
            to_json(&kind),
            body,
            to_json(what)
        ],
    )?;
    let id = MessageId(db.last_insert_rowid() as u32);
    let entry = Entry::Message {
        how: kind,
        what: Some(what.clone()),
        from: events::who(db, from)?,
        to: to.map(|to| events::who(db, to)).transpose()?,
        step: if to.is_some() { Step::Sent } else { Step::Held },
        via: stamp.via,
        gate: stamp.gate,
    };
    events::write(db, &entry, Some(id))?;
    Ok(id)
}

pub(super) fn enqueue_human(
    db: &Connection,
    to: NodeId,
    what: &MessageKind,
    body: &str,
) -> Result<MessageId, Refusal> {
    let name = super::name_of(db, to)?;
    check_room(db, to, &name)?;
    db.execute(
        "INSERT INTO messages (recipient, sender, sender_name, kind, body, what) VALUES (?1, NULL, ?2, ?3, ?4, ?5)",
        params![to.0, HUMAN, to_json(&Kind::Queue), body, to_json(what)],
    )?;
    Ok(MessageId(db.last_insert_rowid() as u32))
}

pub(super) fn enqueue_from_loom(
    db: &Connection,
    to: NodeId,
    kind: Kind,
    what: &MessageKind,
    body: &str,
    gate: Option<GateTrace>,
) -> rusqlite::Result<MessageId> {
    db.execute(
        "INSERT INTO messages (recipient, sender, sender_name, kind, body, what) VALUES (?1, NULL, ?2, ?3, ?4, ?5)",
        params![to.0, LOOM, to_json(&kind), body, to_json(what)],
    )?;
    let id = MessageId(db.last_insert_rowid() as u32);
    let entry = Entry::Message {
        how: kind,
        what: Some(what.clone()),
        from: events::Who {
            id: None,
            name: LOOM.to_owned(),
        },
        to: Some(events::who(db, to)?),
        step: Step::Sent,
        via: None,
        gate,
    };
    events::write(db, &entry, Some(id))?;
    Ok(id)
}

fn read_delivery(row: &rusqlite::Row<'_>) -> rusqlite::Result<Delivery> {
    Ok(Delivery {
        id: MessageId(row.get(0)?),
        from: row.get(1)?,
        kind: from_json(row, 2)?,
        body: row.get(3)?,
        what: optional_json(row, 4)?,
    })
}

pub const PARENT: &str = "parent";

fn node_id(db: &Connection, from: NodeId, to: &str) -> Result<Option<u32>, Refusal> {
    if to == PARENT {
        return Ok(Some(parent_of(db, from)?.map_or(0, |p| p.0)));
    }
    Ok(to.strip_prefix('#').unwrap_or(to).parse::<u32>().ok())
}

fn address(db: &Connection, from: NodeId, to: &str) -> Result<NodeId, Refusal> {
    if let Some(id) = node_id(db, from, to)? {
        let exists: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM nodes WHERE id = ?1 AND id != ?2)",
            params![id, from.0],
            |row| row.get(0),
        )?;
        return if exists {
            Ok(NodeId(id))
        } else {
            Err(Refusal::NotFound(to.to_owned()))
        };
    }
    match related(db, from, to) {
        Err(Refusal::NotRelated(_)) => {}
        found => return found.map(|(id, _)| id),
    }
    let named = db
        .prepare(&format!(
            "SELECT id, {REACHABLE} FROM nodes WHERE name = ?1 AND id != ?2 ORDER BY id"
        ))?
        .query_map(params![to, from.0], |row| {
            Ok((NodeId(row.get(0)?), row.get::<_, bool>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let live: Vec<NodeId> = named
        .iter()
        .filter(|(_, reachable)| *reachable)
        .map(|(id, _)| *id)
        .collect();
    match live[..] {
        [one] => Ok(one),
        [] if named.is_empty() => Err(Refusal::NotFound(to.to_owned())),
        [] => Err(Refusal::Ended(to.to_owned())),
        _ => Err(ambiguous(db, to, &live)?),
    }
}

fn steerable(
    db: &Connection,
    from: NodeId,
    to: &str,
) -> Result<(NodeId, Option<AgentId>), Refusal> {
    match related(db, from, to) {
        Err(Refusal::NotRelated(_)) => {}
        found => return found,
    }
    let id = node_id(db, from, to)?;
    db.query_row(
        "SELECT n.id, n.agent FROM nodes n, nodes me WHERE me.id = ?1 AND n.id != me.id
         AND (CASE WHEN ?3 IS NULL THEN n.name = ?2 ELSE n.id = ?3 END)
         AND n.parent = me.parent AND n.review = me.review",
        params![from.0, to, id],
        |r| Ok((NodeId(r.get(0)?), from_json(r, 1)?)),
    )
    .optional()?
    .ok_or_else(|| Refusal::NotRelated(to.to_owned()))
}

pub(super) fn related(
    db: &Connection,
    from: NodeId,
    to: &str,
) -> Result<(NodeId, Option<AgentId>), Refusal> {
    let id = node_id(db, from, to)?;
    let found = db.prepare(
        "SELECT id, agent FROM nodes WHERE (CASE WHEN ?3 IS NULL THEN name = ?2 ELSE id = ?3 END)
         AND (parent = ?1 OR id = (SELECT parent FROM nodes WHERE id = ?1)) ORDER BY id",
    )?.query_map(params![from.0, to, id], |row| Ok((NodeId(row.get(0)?), from_json(row, 1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    match found.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(Refusal::NotRelated(to.to_owned())),
        _ => {
            let ids: Vec<NodeId> = found.iter().map(|(id, _)| *id).collect();
            Err(ambiguous(db, to, &ids)?)
        }
    }
}
