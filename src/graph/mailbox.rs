//! 메일함: 부모와 자식 사이 메시지. 전달 방식(`Kind`)마다 꺼내는 지점이 다르다.
//! 부모 없는 노드가 부모에게 보내는 메시지는 보관했다가 새 부모에게 전한다.
//! 근거: docs/design.md 「전달 지점」.

use std::pin::pin;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::events::{self, Entry, Step};
use super::{
    Graph, MAILBOX_LIMIT, NodeId, NodeState, Refusal, SUMMARY_LIMIT_BYTES, from_json, name_of,
    parent_of, set_state, state_of, to_json,
};
use crate::profile::{AgentId, BRIEF_LIMIT_BYTES};

/// 메시지 전달 방식. 근거: docs/design.md 「명령」.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
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

/// 꺼낸 메시지를 받는 쪽이 확인하지 않으면 이 시간 뒤에 다시 준다.
/// 근거: 꺼내고 확인하기까지 가장 오래 걸리는 쪽은 Claude 훅이다. Claude는 훅을 10초(`timeout`)에 끝낸다. 그 3배를 둔다.
/// 다음 호출 때 바로 다시 주지 않는 이유: Claude 노드는 백그라운드 `loom wait`와 훅이 같은 메일함을 함께 꺼낸다.
/// 바로 다시 주면 한쪽이 확인하기 전에 다른 쪽이 같은 메시지를 또 받는다.
pub const REDELIVER_AFTER: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct MessageId(pub u32);

/// 받는 노드에 넘기는 메시지 하나. 같은 메시지가 두 번 오면 `id`가 같다.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Delivery {
    pub id: MessageId,
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

    /// `from`의 부모나 자식 가운데 이름이 `to`인 노드에게 보내고 받은 노드를 돌려준다.
    /// `from`에 부모가 없고 그래프에 `to` 노드가 없으면 지운 부모에게 보내는 큐 메시지로 보고 보관한다. 이때는 `None`이다.
    pub fn send(
        &self,
        from: NodeId,
        to: &str,
        kind: Kind,
        body: &str,
    ) -> Result<Option<NodeId>, Refusal> {
        if body.len() > BRIEF_LIMIT_BYTES {
            return Err(Refusal::BriefTooLong(body.len()));
        }
        let found = self.recipient(from, to);
        let mut db = self.lock();
        let to_id = match found {
            Ok((to_id, _)) => to_id,
            // 조정·중단은 받는 노드의 프로필에 따라 넣는 방법이 다르다. 받을 노드가 정해지지 않았으므로 보관하지 않는다.
            Err(Refusal::NotRelated(_))
                if kind == Kind::Queue
                    && parent_of(&db, from)?.is_none()
                    && !name_exists(&db, to)? =>
            {
                check_held_room(&db, from)?;
                enqueue(&db, from, None, kind, body)?;
                self.publish_node(&db, from)?;
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        if matches!(state_of(&db, to_id)?, NodeState::Exited { .. }) {
            return Err(Refusal::Ended(to.to_owned()));
        }
        check_room(&db, to_id, to)?;
        let tx = db.transaction()?;
        enqueue(&tx, from, Some(to_id), kind, body)?;
        tx.commit()?;
        self.publish_node(&db, to_id)?;
        let child = if parent_of(&db, from)? == Some(to_id) {
            from
        } else {
            to_id
        };
        self.publish_message(child);
        Ok(Some(to_id))
    }

    /// 가장 먼저 온 메시지 하나를 꺼낸다. 없으면 올 때까지 기다린다. 받은 쪽은 `ack`로 확인한다.
    pub async fn wait(&self, node: NodeId) -> Result<Delivery, Refusal> {
        loop {
            // 확인하기 전에 깨우기 신호를 받을 준비를 한다. 그래야 확인과 기다림 사이에 온 메시지를 놓치지 않는다.
            let mut notified = pin!(self.mail_changed());
            notified.as_mut().enable();
            if let Some(delivery) = self.take(node, Kind::ALL, 0)?.pop() {
                return Ok(delivery);
            }
            // 확인 없이 둔 메시지는 새 메시지 알림 없이 다시 꺼낼 수 있게 된다.
            let _ = tokio::time::timeout(REDELIVER_AFTER, notified).await;
        }
    }

    /// `kinds` 가운데 하나인 메시지를 온 순서로 꺼내 전달 중으로 표시한다.
    /// 전달 중인 메시지는 `REDELIVER_AFTER`가 지나야 다시 꺼낸다. 받은 쪽이 `ack`로 확인하면 다시 꺼내지 않는다.
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
                "SELECT id, sender_name, kind, body FROM messages
                 WHERE recipient = ?1 AND delivered = 0
                 AND (sent_at IS NULL OR sent_at <= unixepoch() - ?3)
                 AND kind IN (SELECT value FROM json_each(?2)) ORDER BY id",
            )?
            .query_map(
                params![node.0, kinds, REDELIVER_AFTER.as_secs() as i64],
                |row| {
                    Ok(Delivery {
                        id: MessageId(row.get(0)?),
                        from: row.get(1)?,
                        kind: from_json(row, 2)?,
                        body: row.get(3)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut taken = Vec::new();
        let mut total = 0;
        for delivery in queued {
            total += delivery.body.len();
            if !taken.is_empty() && total > limit_bytes {
                break;
            }
            tx.execute(
                "UPDATE messages SET sent_at = unixepoch() WHERE id = ?1",
                [delivery.id.0],
            )?;
            taken.push(delivery);
        }
        tx.commit()?;
        Ok(taken)
    }

    /// 받는 쪽이 `ids` 메시지를 받았다. 다시 꺼내지 않는다. 다른 노드의 메시지 id는 무시한다.
    pub fn ack(&self, node: NodeId, ids: &[MessageId]) -> Result<(), Refusal> {
        let mut db = self.lock();
        let ids = serde_json::to_string(&ids.iter().map(|id| id.0).collect::<Vec<_>>())
            .expect("숫자 목록은 늘 직렬화된다");
        let tx = db.transaction()?;
        let acked = tx
            .prepare(
                "UPDATE messages SET delivered = 1
                 WHERE recipient = ?1 AND delivered = 0 AND id IN (SELECT value FROM json_each(?2))
                 RETURNING id, kind, sender, sender_name",
            )?
            .query_map(params![node.0, ids], |row| {
                Ok((
                    MessageId(row.get(0)?),
                    from_json::<Kind>(row, 1)?,
                    row.get::<_, Option<u32>>(2)?.map(NodeId),
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let to = events::who(&tx, node)?;
        for (id, how, sender, name) in &acked {
            let entry = Entry::Message {
                how: *how,
                from: events::Who {
                    id: *sender,
                    name: name.clone(),
                },
                to: Some(to.clone()),
                step: Step::Delivered,
            };
            events::write(&tx, &entry, Some(*id))?;
        }
        tx.commit()?;
        if !acked.is_empty() {
            self.publish_node(&db, node)?;
        }
        Ok(())
    }

    /// 노드를 `done`으로 바꾸고 요약을 부모에게 보낸다. 부모가 없으면 보관한다.
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
            check_room(&db, parent, &name_of(&db, parent)?)?;
        }
        let tx = db.transaction()?;
        set_state(
            &tx,
            node,
            &NodeState::Done {
                summary: summary.to_owned(),
            },
        )?;
        enqueue(&tx, node, parent, Kind::Queue, &format!("done: {summary}"))?;
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

fn name_exists(db: &Connection, name: &str) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT EXISTS (SELECT 1 FROM nodes WHERE name = ?1)",
        [name],
        |row| row.get(0),
    )
}

/// 부모 없는 노드가 보관한 메시지도 상한을 둔다. 새 부모가 한꺼번에 받으므로 같은 상한을 쓴다.
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

/// 상한을 보지 않고 넣는다. 상한은 부르는 쪽이 `check_room`으로 본다. `to`가 없으면 `from`이 보관한다.
pub(super) fn enqueue(
    db: &Connection,
    from: NodeId,
    to: Option<NodeId>,
    kind: Kind,
    body: &str,
) -> rusqlite::Result<MessageId> {
    db.execute(
        "INSERT INTO messages (recipient, sender, sender_name, kind, body)
         VALUES (?1, ?2, (SELECT name FROM nodes WHERE id = ?2), ?3, ?4)",
        params![to.map(|to| to.0), from.0, to_json(&kind), body],
    )?;
    let id = MessageId(db.last_insert_rowid() as u32);
    let entry = Entry::Message {
        how: kind,
        from: events::who(db, from)?,
        to: to.map(|to| events::who(db, to)).transpose()?,
        step: if to.is_some() { Step::Sent } else { Step::Held },
    };
    events::write(db, &entry, Some(id))?;
    Ok(id)
}
