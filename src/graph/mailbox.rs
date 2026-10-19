//! 메일함: 부모와 자식 사이 메시지. 큐 방식(`loom send`, `loom wait`, `loom done`)만 둔다.

use std::pin::pin;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::{
    Graph, MAILBOX_LIMIT, NodeId, NodeState, Refusal, SUMMARY_LIMIT_BYTES, parent_of, set_state,
    state_of,
};
use crate::profile::BRIEF_LIMIT_BYTES;

/// `loom wait`가 받는 메시지 하나.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Delivery {
    pub from: String,
    pub body: String,
}

impl Graph {
    /// `from`의 부모나 자식 가운데 이름이 `to`인 노드에게 보낸다.
    pub fn send(&self, from: NodeId, to: &str, body: &str) -> Result<(), Refusal> {
        if body.len() > BRIEF_LIMIT_BYTES {
            return Err(Refusal::BriefTooLong(body.len()));
        }
        let mut db = self.lock();
        // 자식 이름은 형제끼리, 그리고 부모 이름과 겹치지 않는다. 그래서 많아야 한 노드가 맞는다.
        let found: Option<u32> = db
            .query_row(
                "SELECT id FROM nodes WHERE name = ?2
                 AND (parent = ?1 OR id = (SELECT parent FROM nodes WHERE id = ?1))",
                params![from.0, to],
                |row| row.get(0),
            )
            .optional()?;
        let Some(to_id) = found.map(NodeId) else {
            return Err(Refusal::NotRelated(to.to_owned()));
        };
        if matches!(state_of(&db, to_id)?, NodeState::Exited { .. }) {
            return Err(Refusal::Ended(to.to_owned()));
        }
        check_room(&db, to_id, to)?;
        let tx = db.transaction()?;
        enqueue(&tx, from, to_id, body)?;
        tx.commit()?;
        self.publish_node(&db, to_id)?;
        let child = if parent_of(&db, from)? == Some(to_id) {
            from
        } else {
            to_id
        };
        self.publish_message(child);
        Ok(())
    }

    /// 가장 먼저 온 `queued` 메시지를 `delivered`로 바꾸고 돌려준다. 없으면 올 때까지 기다린다.
    pub async fn wait(&self, node: NodeId) -> Result<Delivery, Refusal> {
        loop {
            // 확인하기 전에 깨우기 신호를 받을 준비를 한다. 그래야 확인과 기다림 사이에 온 메시지를 놓치지 않는다.
            let mut notified = pin!(self.mail.notified());
            notified.as_mut().enable();
            if let Some(delivery) = self.take(node)? {
                return Ok(delivery);
            }
            notified.await;
        }
    }

    pub(super) fn take(&self, node: NodeId) -> Result<Option<Delivery>, Refusal> {
        let db = self.lock();
        let found: Option<(u32, String, String)> = db
            .query_row(
                "SELECT m.id, s.name, m.body FROM messages m JOIN nodes s ON s.id = m.sender
                 WHERE m.recipient = ?1 AND m.delivered = 0 ORDER BY m.id LIMIT 1",
                [node.0],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((id, from, body)) = found else {
            return Ok(None);
        };
        db.execute("UPDATE messages SET delivered = 1 WHERE id = ?1", [id])?;
        self.publish_node(&db, node)?;
        Ok(Some(Delivery { from, body }))
    }

    /// 노드를 `done`으로 바꾸고, 부모가 있으면 요약을 보낸다.
    pub fn done(&self, node: NodeId, summary: &str) -> Result<(), Refusal> {
        if summary.len() > SUMMARY_LIMIT_BYTES {
            return Err(Refusal::SummaryTooLong(summary.len()));
        }
        let mut db = self.lock();
        if state_of(&db, node)? != NodeState::Running {
            return Err(Refusal::AlreadyDone);
        }
        let parent = parent_of(&db, node)?;
        if let Some(parent) = parent {
            let name: String =
                db.query_row("SELECT name FROM nodes WHERE id = ?1", [parent.0], |row| {
                    row.get(0)
                })?;
            check_room(&db, parent, &name)?;
        }
        let tx = db.transaction()?;
        set_state(
            &tx,
            node,
            &NodeState::Done {
                summary: summary.to_owned(),
            },
        )?;
        if let Some(parent) = parent {
            enqueue(&tx, node, parent, &format!("done: {summary}"))?;
        }
        tx.commit()?;
        self.publish_node(&db, node)?;
        if let Some(parent) = parent {
            self.publish_node(&db, parent)?;
            self.publish_message(node);
        }
        Ok(())
    }
}

fn check_room(db: &Connection, to: NodeId, name: &str) -> Result<(), Refusal> {
    let queued: u32 = db.query_row(
        "SELECT count(*) FROM messages WHERE recipient = ?1 AND delivered = 0",
        [to.0],
        |row| row.get(0),
    )?;
    if queued >= MAILBOX_LIMIT {
        return Err(Refusal::MailboxFull(name.to_owned()));
    }
    Ok(())
}

/// 상한을 보지 않고 넣는다. 상한은 부르는 쪽이 `check_room`으로 본다.
pub(super) fn enqueue(
    db: &Connection,
    from: NodeId,
    to: NodeId,
    body: &str,
) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO messages (recipient, sender, body) VALUES (?1, ?2, ?3)",
        params![to.0, from.0, body],
    )?;
    Ok(())
}
