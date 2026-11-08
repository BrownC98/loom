use rusqlite::OptionalExtension;

use super::{Graph, mailbox};
use api::graph::messages::{Kind, MessageKind};
use api::ids::NodeId;

const NO_WAKE_SOURCE: &str = r#"n.process IS NULL AND n.state ->> 'kind' != 'done' AND n.wait IS NULL
     AND EXISTS (SELECT 1 FROM agent_states s WHERE s.node = n.id AND s.state ->> 'kind' = 'listening')
     AND NOT EXISTS (SELECT 1 FROM relaunch_plans p WHERE p.node = n.id)
     AND NOT EXISTS (SELECT 1 FROM messages m WHERE m.recipient = n.id AND m.delivered = 0)
     AND NOT EXISTS (SELECT 1 FROM nodes c WHERE c.parent = n.id AND c.state ->> 'kind' != 'done'
         AND (c.process IS NULL OR c.process ->> 'kind' IN ('asleep', 'hostGone')))
     AND NOT EXISTS (SELECT 1 FROM questions q WHERE q.node = n.id)
     AND NOT EXISTS (SELECT 1 FROM messages a WHERE a.sender = n.id AND a.what ->> 'kind' = 'ask'
         AND a.recipient IS NOT NULL AND NOT EXISTS (SELECT 1 FROM messages r
             WHERE r.sender = a.recipient AND r.recipient = n.id AND r.id > a.id))
     AND NOT EXISTS (SELECT 1 FROM gates g WHERE g.owner = n.id AND g.state NOT IN ('"working"', '"merged"'))
     AND (n.review IS NULL OR EXISTS (SELECT 1 FROM gates g
         WHERE g.repo = n.repo AND g.branch = n.review AND g.state = '"ready"'
         AND NOT EXISTS (SELECT 1 FROM gate_votes v WHERE v.repo = g.repo AND v.branch = g.branch
             AND v.reviewer = n.id AND v.vote IS NOT NULL AND v.head = g.head)))
     AND (SELECT m.what ->> 'kind' FROM messages m WHERE m.recipient = n.id OR m.sender = n.id
         ORDER BY m.id DESC LIMIT 1) IS NOT 'idle'"#;

impl Graph {
    pub fn wake_idle(&self, node: NodeId, body: &str) -> rusqlite::Result<bool> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let due = tx
            .query_row(
                &format!("SELECT {NO_WAKE_SOURCE} FROM nodes n WHERE n.id = ?1"),
                [node.0],
                |row| row.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(false);
        if !due {
            return Ok(false);
        }
        mailbox::enqueue_from_loom(&tx, node, Kind::Queue, &MessageKind::Idle, body, None)?;
        tx.commit()?;
        drop(db);
        self.publish_nodes();
        self.mail.notify_waiters();
        Ok(true)
    }
}
