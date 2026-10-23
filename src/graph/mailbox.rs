//! 메일함: 노드 사이 메시지. 큐 메시지는 어느 노드에나 가고, 조정·중단은 부모와 자식 사이에만 간다. 전달 방식(`Kind`)마다 꺼내는 지점이 다르다.
//! 부모 없는 노드가 부모에게 보내는 메시지는 보관했다가 새 부모에게 전한다.
//! 근거: docs/design.md 「전달 지점」.

use std::path::PathBuf;
use std::pin::pin;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::events::{self, Entry, GateTrace, Step, Via};
use super::waits;
use super::{
    Graph, HUMAN, MAILBOX_LIMIT, NodeId, NodeState, Refusal, SUMMARY_LIMIT_BYTES, Toward,
    ambiguous, from_json, parent_of, set_state, state_of, to_json,
};
use crate::link_text::bare_link;
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
/// 다음 호출 때 바로 다시 주지 않는 이유: Claude 노드는 `loom wait`와 훅들이 같은 메일함을 함께 꺼낸다.
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
    #[serde(default)]
    pub reply_to: Option<MessageId>,
}

/// 보낸 메시지 번호와 받는 노드. 부모가 없어 보관하면 `recipient`가 없다.
#[derive(Debug, Serialize, Deserialize)]
pub struct Sent {
    pub id: MessageId,
    pub recipient: Option<NodeId>,
}

struct Outgoing<'a> {
    kind: Kind,
    body: &'a str,
    reply_to: Option<MessageId>,
    via: Option<&'a Via>,
}

/// 메시지 이벤트에 붙이는 흔적: 들어온 입구와, 관문 동사가 보냈으면 관문 흔적.
#[derive(Default)]
pub(super) struct Stamp {
    pub via: Option<Via>,
    pub gate: Option<GateTrace>,
}

impl Graph {
    /// 조정·중단을 받을 부모·자식을 이름이나 `#노드번호`로 찾는다. 여러 노드가 맞으면 거절한다.
    pub fn related(&self, from: NodeId, to: &str) -> Result<(NodeId, Option<AgentId>), Refusal> {
        related(&self.lock(), from, to)
    }

    pub fn send(
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
                body,
                reply_to: None,
                via,
            },
            |_| {},
        )
    }

    /// Claude 셸은 검증과 큐 쓰기가 끝난 뒤, 메시지를 꺼낼 수 있기 전에 끝낸다.
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
                body,
                reply_to: None,
                via,
            },
            stop,
        )
    }

    /// 내가 받은 메시지의 보낸 노드에게 답한다.
    pub fn reply(
        &self,
        from: NodeId,
        request: MessageId,
        body: &str,
        via: Option<&Via>,
    ) -> Result<Sent, Refusal> {
        let sender: Option<u32> = self
            .lock()
            .query_row(
                "SELECT sender FROM messages WHERE id = ?1 AND recipient = ?2",
                params![request.0, from.0],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        let sender = sender.ok_or_else(|| Refusal::NotFound(format!("메시지 #{}", request.0)))?;
        self.post(
            from,
            &format!("#{sender}"),
            Outgoing {
                kind: Kind::Queue,
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
            Kind::Steer | Kind::Interrupt => related(&db, from, to).map(|(id, _)| id),
        };
        let to_id = match found {
            Ok(to_id) => Some(to_id),
            // 지운 부모의 이름과 `parent`는 그래프에 없다. 부모 없는 노드가 그 이름으로 보내면 보관한다.
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
            let exited: bool = db.query_row(
                "SELECT process_exited FROM nodes WHERE id = ?1",
                [to_id.0],
                |row| row.get(0),
            )?;
            if exited {
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
        let id = enqueue(&tx, from, to_id, kind, body, stamp)?;
        if let Some(request) = reply_to {
            // 답변을 쓰는 동안 원래 메시지의 보낸 노드와 받은 노드가 바뀌지 않았는지 본다.
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

    /// 커밋한 뒤 부른다: 받는 노드와 메시지가 간 간선을 화면에 알리고 기다리는 `wait`를 깨운다.
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

    /// 답변 대기는 훅이 이미 받은 답도 찾는다. 다른 소비자 때문에 답을 놓치지 않는다.
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
                    "SELECT id, sender_name, kind, body, reply_to FROM messages
                     WHERE recipient = ?1 AND reply_to = ?2 ORDER BY id LIMIT 1",
                    params![node.0, request.0],
                    read_delivery,
                )
                .optional()?;
            if let Some(delivery) = delivery {
                return Ok(delivery);
            }
            let _ = tokio::time::timeout(REDELIVER_AFTER, notified).await;
        }
    }

    /// 메시지가 올 때까지 기다렸다가 `take`처럼 꺼낸다. 버려도 꺼낸 메시지를 잃지 않는다. 꺼내기와 돌려주기 사이에 기다림이 없다.
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
                "SELECT id, sender_name, kind, body, reply_to FROM messages
                 WHERE recipient = ?1 AND delivered = 0
                 AND (sent_at IS NULL OR sent_at <= unixepoch() - ?3)
                 AND kind IN (SELECT value FROM json_each(?2)) ORDER BY id",
            )?
            .query_map(
                params![node.0, kinds, REDELIVER_AFTER.as_secs() as i64],
                read_delivery,
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
        let resumed = !taken.is_empty() && resume(&tx, node)?;
        tx.commit()?;
        if resumed {
            self.publish_node(&db, node)?;
        }
        Ok(taken)
    }

    /// 사람이 노드에 입력했다. 끝낸 노드면 다시 일한다.
    pub fn human_input(&self, node: NodeId) -> Result<(), Refusal> {
        let db = self.lock();
        if resume(&db, node)? {
            self.publish_node(&db, node)?;
        }
        Ok(())
    }

    /// 사람이 노드 카드의 「안 읽음」을 눌렀다. 아직 꺼내지 않은 큐 메시지를 조정으로 바꾼다.
    /// 일하는 노드는 다음 단계 사이에, 쉬는 노드는 새 턴으로 받는다(2026-10-22 사람 지시 "칩을 클릭하면 수동으로 주입되게").
    pub fn steer_queued(&self, node: NodeId) -> Result<(), Refusal> {
        self.lock().execute(
            "UPDATE messages SET kind = ?2
             WHERE recipient = ?1 AND delivered = 0 AND sent_at IS NULL AND kind = ?3",
            params![node.0, to_json(&Kind::Steer), to_json(&Kind::Queue)],
        )?;
        self.mail.notify_waiters();
        Ok(())
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
                via: None,
                gate: None,
            };
            events::write(&tx, &entry, Some(*id))?;
        }
        tx.commit()?;
        if !acked.is_empty() {
            self.publish_node(&db, node)?;
        }
        Ok(())
    }

    /// 노드를 `done`으로 바꾸고 요약을 부모에게 보낸다. 이미 `done`이어도 새 요약으로 다시 보낸다.
    /// 근거: 다시 깨기 전에 부른 둘째 `loom done`을 거절하자 결과가 부모에게 가지 않았다(2026-10-22 기록에서 거절 23번). 부모가 없으면 받은편지함에 완료를 건다(`waits::put_done`).
    /// 부모 없는 노드는 `asked`(그 일을 맡긴 사람의 지시)가 있어야 한다. `images`는 사람이 그 지시에 준 그림 파일이다. `doc`(`--doc`)은 완료 줄의 문서 링크다.
    /// 돌려준 값은 프로세스가 아직 사는 자식 이름이다. 회수하지 않은 자식이 고아로 남지 않게 부르는 쪽이 알린다.
    pub fn done(
        &self,
        node: NodeId,
        summary: &str,
        asked: Option<&str>,
        images: &[PathBuf],
        doc: Option<waits::WaitDoc>,
        via: Option<&Via>,
    ) -> Result<Vec<String>, Refusal> {
        if summary.len() > SUMMARY_LIMIT_BYTES {
            return Err(Refusal::SummaryTooLong(summary.len()));
        }
        let bare = bare_link(summary);
        let mut db = self.lock();
        let parent = parent_of(&db, node)?;
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
            // 받은편지함이 사람에게 알린다. 다음 부모에게 줄 보관 메시지로 또 두면 「보관 1」만 남았다(2026-10-22 사람 질문 "위에 뜬 보관 1은 뭔가").
            Some(asked) if parent.is_none() => {
                waits::put_done(&tx, node, asked, summary, images, doc)?
            }
            _ => {
                let stamp = Stamp {
                    via: via.cloned(),
                    gate: None,
                };
                let body = format!("done: {summary}");
                enqueue(&tx, node, parent, Kind::Queue, &body, stamp)?;
            }
        }
        let alive_children = tx
            .prepare("SELECT name FROM nodes WHERE parent = ?1 AND process_exited = 0 ORDER BY id")?
            .query_map([node.0], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        tx.commit()?;
        self.publish_node(&db, node)?;
        if let Some(parent) = parent {
            self.publish_node(&db, parent)?;
            self.publish_message(node, Toward::Parent);
        }
        Ok(alive_children)
    }
}

/// `done` 노드가 새 일(메시지, 사람 입력)을 받으면 `running`으로 돌린다. 다시 끝내면 `loom done`을 또 부른다.
/// 근거: 피드백으로 다시 일하는 노드가 「완료」로 보였다(2026-10-22 사람 보고 "작업중인데 완료 상태로 표시되는 문제").
fn resume(db: &Connection, node: NodeId) -> rusqlite::Result<bool> {
    let done = matches!(state_of(db, node), Ok(NodeState::Done { .. }));
    if done {
        set_state(db, node, &NodeState::Running)?;
    }
    Ok(done)
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
    stamp: Stamp,
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
        via: stamp.via,
        gate: stamp.gate,
    };
    events::write(db, &entry, Some(id))?;
    Ok(id)
}

/// 사람이 받은편지함에서 보낸 큐 메시지(답, 추가 지시, 답 없이 닫힘). 보낸 노드가 없으므로 `sender`는 비우고 이름은 `HUMAN`이다.
/// 메시지 이벤트는 쓰지 않는다. 부르는 쪽이 같은 일을 알림 이벤트(답·추가 지시·답 없이 닫힘)로 한 줄 쓴다. 두 줄로 쓰면 이벤트 목록에 같은 답이 메시지로 한 번 더 보였다(2026-10-22 사람 지시 "가운데도 알림 이벤트로 나와야 하는거 아님?").
pub(super) fn enqueue_human(db: &Connection, to: NodeId, body: &str) -> Result<MessageId, Refusal> {
    let name = super::name_of(db, to)?;
    check_room(db, to, &name)?;
    db.execute(
        "INSERT INTO messages (recipient, sender, sender_name, kind, body) VALUES (?1, NULL, ?2, ?3, ?4)",
        params![to.0, HUMAN, to_json(&Kind::Queue), body],
    )?;
    Ok(MessageId(db.last_insert_rowid() as u32))
}

fn read_delivery(row: &rusqlite::Row<'_>) -> rusqlite::Result<Delivery> {
    Ok(Delivery {
        id: MessageId(row.get(0)?),
        from: row.get(1)?,
        kind: from_json(row, 2)?,
        body: row.get(3)?,
        reply_to: row.get::<_, Option<u32>>(4)?.map(MessageId),
    })
}

/// 부모를 가리키는 받는 쪽 이름.
pub const PARENT: &str = "parent";

/// 이름 대신 쓰는 노드 번호. `#12`·`12`는 번호, `parent`는 부르는 노드의 부모다. 부모가 없으면 없는 번호(0)다.
/// 근거: 에이전트가 `#96` 대신 `96`(14번), 부모 이름 대신 `parent`·`#parent`(5번)로 보내 거절됐다(2026-10-20~22 이벤트 기록).
/// 숫자만인 이름과 `parent`는 노드 이름으로 받지 않는다(`nodes::checked_name`).
fn node_id(db: &Connection, from: NodeId, to: &str) -> Result<Option<u32>, Refusal> {
    if to == PARENT {
        return Ok(Some(parent_of(db, from)?.map_or(0, |p| p.0)));
    }
    Ok(to.strip_prefix('#').unwrap_or(to).parse::<u32>().ok())
}

/// 큐 메시지를 받을 노드. 번호(`node_id`)는 그 노드다. 이름은 부모·자식을 먼저 보고, 없으면 그래프 전체에서 프로세스가 사는 노드를 본다.
/// 근거: docs/design.md 「메시징 › 원칙」.
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
        .prepare("SELECT id, process_exited FROM nodes WHERE name = ?1 AND id != ?2 ORDER BY id")?
        .query_map(params![to, from.0], |row| {
            Ok((NodeId(row.get(0)?), row.get::<_, bool>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let live: Vec<NodeId> = named
        .iter()
        .filter(|(_, exited)| !exited)
        .map(|(id, _)| *id)
        .collect();
    match live[..] {
        [one] => Ok(one),
        [] if named.is_empty() => Err(Refusal::NotFound(to.to_owned())),
        [] => Err(Refusal::Ended(to.to_owned())),
        _ => Err(ambiguous(db, to, &live)?),
    }
}

/// 부모·자식 가운데 이름이나 `#노드번호`가 맞는 노드. 여러 노드가 맞으면 거절한다.
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
