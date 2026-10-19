//! 메일함: 부모와 자식 사이 메시지. 전달 방식(`Kind`)마다 꺼내는 지점이 다르다.
//! 근거: docs/design.md 「전달 지점」.

use std::pin::pin;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::{
    Graph, MAILBOX_LIMIT, NodeId, NodeState, Refusal, SUMMARY_LIMIT_BYTES, from_json, parent_of,
    set_state, state_of, to_json,
};
use crate::profile::{AgentId, BRIEF_LIMIT_BYTES};

/// 메시지 전달 방식. 근거: docs/design.md 「명령」.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// `loom send`: 받는 노드의 턴을 끊지 않는다.
    Queue,
    /// `loom steer`: 턴을 멈추지 않고 다음 도구 호출 사이에 넣는다.
    Steer,
    /// `loom interrupt`: 지금 동작을 멈추고 바로 따르게 한다.
    Interrupt,
}

impl Kind {
    pub const ALL: &[Kind] = &[Kind::Queue, Kind::Steer, Kind::Interrupt];
}

/// 받는 노드에 넘기는 메시지 하나.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Delivery {
    pub from: String,
    pub kind: Kind,
    pub body: String,
}

impl Graph {
    /// `from`의 부모나 자식 가운데 이름이 `to`인 노드와 그 에이전트를 찾는다.
    pub fn recipient(&self, from: NodeId, to: &str) -> Result<(NodeId, Option<AgentId>), Refusal> {
        let db = self.lock();
        // 자식 이름은 형제끼리, 그리고 부모 이름과 겹치지 않는다. 그래서 많아야 한 노드가 맞는다.
        let found = db
            .query_row(
                "SELECT id, agent FROM nodes WHERE name = ?2
                 AND (parent = ?1 OR id = (SELECT parent FROM nodes WHERE id = ?1))",
                params![from.0, to],
                |row| Ok((NodeId(row.get(0)?), from_json(row, 1)?)),
            )
            .optional()?;
        found.ok_or_else(|| Refusal::NotRelated(to.to_owned()))
    }

    /// `from`의 부모나 자식 가운데 이름이 `to`인 노드에게 보낸다. 받은 노드를 돌려준다.
    pub fn send(&self, from: NodeId, to: &str, kind: Kind, body: &str) -> Result<NodeId, Refusal> {
        if body.len() > BRIEF_LIMIT_BYTES {
            return Err(Refusal::BriefTooLong(body.len()));
        }
        let (to_id, _) = self.recipient(from, to)?;
        let mut db = self.lock();
        if matches!(state_of(&db, to_id)?, NodeState::Exited { .. }) {
            return Err(Refusal::Ended(to.to_owned()));
        }
        check_room(&db, to_id, to)?;
        let tx = db.transaction()?;
        enqueue(&tx, from, to_id, kind, body)?;
        tx.commit()?;
        self.publish_node(&db, to_id)?;
        let child = if parent_of(&db, from)? == Some(to_id) {
            from
        } else {
            to_id
        };
        self.publish_message(child);
        Ok(to_id)
    }

    /// 가장 먼저 온 `queued` 메시지 하나를 꺼낸다. 없으면 올 때까지 기다린다.
    pub async fn wait(&self, node: NodeId) -> Result<Delivery, Refusal> {
        loop {
            // 확인하기 전에 깨우기 신호를 받을 준비를 한다. 그래야 확인과 기다림 사이에 온 메시지를 놓치지 않는다.
            let mut notified = pin!(self.mail.notified());
            notified.as_mut().enable();
            if let Some(delivery) = self.take(node, Kind::ALL, 0)?.pop() {
                return Ok(delivery);
            }
            notified.await;
        }
    }

    /// `kinds` 가운데 하나인 `queued` 메시지를 온 순서로 꺼내 `delivered`로 바꾼다.
    /// 첫 메시지는 늘 꺼낸다. 그 뒤 메시지는 본문 합이 `limit_bytes` 이하일 때만 꺼낸다.
    pub fn take(
        &self,
        node: NodeId,
        kinds: &[Kind],
        limit_bytes: usize,
    ) -> Result<Vec<Delivery>, Refusal> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let kinds = serde_json::to_string(&kinds.iter().map(to_json).collect::<Vec<_>>())
            .expect("문자열 목록은 늘 직렬화된다");
        let queued = tx
            .prepare(
                "SELECT m.id, s.name, m.kind, m.body FROM messages m JOIN nodes s ON s.id = m.sender
                 WHERE m.recipient = ?1 AND m.delivered = 0
                 AND m.kind IN (SELECT value FROM json_each(?2)) ORDER BY m.id",
            )?
            .query_map(params![node.0, kinds], |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    Delivery {
                        from: row.get(1)?,
                        kind: from_json(row, 2)?,
                        body: row.get(3)?,
                    },
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut taken = Vec::new();
        let mut total = 0;
        for (id, delivery) in queued {
            total += delivery.body.len();
            if !taken.is_empty() && total > limit_bytes {
                break;
            }
            tx.execute("UPDATE messages SET delivered = 1 WHERE id = ?1", [id])?;
            taken.push(delivery);
        }
        tx.commit()?;
        if !taken.is_empty() {
            self.publish_node(&db, node)?;
        }
        Ok(taken)
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
            enqueue(&tx, node, parent, Kind::Queue, &format!("done: {summary}"))?;
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
    kind: Kind,
    body: &str,
) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO messages (recipient, sender, kind, body) VALUES (?1, ?2, ?3, ?4)",
        params![to.0, from.0, to_json(&kind), body],
    )?;
    Ok(())
}
