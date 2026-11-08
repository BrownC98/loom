use rusqlite::OptionalExtension;

use super::turns::{BACKGROUND, HUMAN_REPLY, STATUS};
use super::{Graph, mailbox};
use api::graph::messages::{Kind, MessageKind};
use api::ids::NodeId;

fn quiet() -> String {
    format!(
        "n.process IS NULL AND n.state ->> 'kind' != 'done' AND n.wait IS NULL AND n.reply IS NOT NULL
     AND (n.turn_flags & {HUMAN_REPLY}) = 0
     AND EXISTS (SELECT 1 FROM agent_states s WHERE s.node = n.id AND s.state ->> 'kind' = 'listening')
     AND NOT EXISTS (SELECT 1 FROM questions q WHERE q.node = n.id)"
    )
}

const MAIL: &str = r#"CASE WHEN EXISTS (SELECT 1 FROM relaunch_plans p WHERE p.node = n.id)
         OR EXISTS (SELECT 1 FROM messages m WHERE m.recipient = n.id AND m.delivered = 0)
     THEN '{"kind":"mail"}' END"#;

const CHILDREN: &str =
    "(SELECT json_object('kind', 'children', 'names', json_group_array(c.name ORDER BY c.id))
     FROM nodes c WHERE c.parent = n.id AND c.state ->> 'kind' != 'done' AND c.review IS NULL
         AND (c.process IS NULL OR c.process ->> 'kind' IN ('asleep', 'hostGone'))
     HAVING count(*) > 0)";

const ANSWER: &str = r#"CASE WHEN EXISTS (SELECT 1 FROM messages a WHERE a.sender = n.id AND a.what ->> 'kind' = 'ask'
         AND a.recipient IS NOT NULL AND NOT EXISTS (SELECT 1 FROM messages r
             WHERE r.sender = a.recipient AND r.recipient = n.id AND r.id > a.id))
     THEN '{"kind":"answer"}' END"#;

const REVIEW: &str = r#"(SELECT json_object('kind', 'review', 'branch', g.branch,
         'judged', (SELECT count(*) FROM gate_votes v WHERE v.repo = g.repo AND v.branch = g.branch
             AND v.vote IS NOT NULL AND v.head = g.head),
         'reviewers', (SELECT count(*) FROM nodes r WHERE r.parent = n.id AND r.review = g.branch
             AND (r.process IS NULL OR r.process ->> 'kind' IN ('asleep', 'hostGone'))))
     FROM gates g WHERE g.owner = n.id AND g.state = '"ready"' ORDER BY g.opened DESC LIMIT 1)"#;

const MERGE: &str = r#"(SELECT json_object('kind', 'merge', 'branch', g.branch) FROM gates g
     WHERE g.owner = n.id AND g.state IN ('"passed"', '"failed"') ORDER BY g.opened DESC LIMIT 1)"#;

const REREVIEW: &str = r#"CASE WHEN n.review IS NOT NULL AND NOT EXISTS (SELECT 1 FROM gates g
         WHERE g.repo = n.repo AND g.branch = n.review AND g.state = '"ready"'
         AND NOT EXISTS (SELECT 1 FROM gate_votes v WHERE v.repo = g.repo AND v.branch = g.branch
             AND v.reviewer = n.id AND v.vote IS NOT NULL AND v.head = g.head))
     THEN json_object('kind', 'rereview', 'branch', n.review) END"#;

const DEPLOY: &str = "(SELECT json_object('kind', 'deploy', 'target', d.target, 'sha', d.sha)
     FROM deploys d WHERE d.root = n.id ORDER BY d.id DESC LIMIT 1)";

fn waited() -> String {
    format!("coalesce({MAIL}, {CHILDREN}, {ANSWER}, {REVIEW}, {MERGE}, {REREVIEW}, {DEPLOY})")
}

fn reason() -> String {
    format!(
        r#"coalesce({}, CASE WHEN (n.turn_flags & {}) != 0 THEN '{{"kind":"job"}}' END)"#,
        waited(),
        STATUS | BACKGROUND
    )
}

pub(super) fn idle_status() -> String {
    format!(
        r#"coalesce({}, CASE WHEN {} THEN '{{"kind":"stalled"}}' END)"#,
        reason(),
        quiet()
    )
}

fn no_wake_source() -> String {
    format!(
        "{} AND {} IS NULL
     AND (SELECT m.what ->> 'kind' FROM messages m WHERE m.recipient = n.id OR m.sender = n.id
         ORDER BY m.id DESC LIMIT 1) IS NOT 'idle'",
        quiet(),
        waited()
    )
}

impl Graph {
    pub fn wake_idle(&self, node: NodeId, body: &str) -> rusqlite::Result<bool> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let due = tx
            .query_row(
                &format!("SELECT {} FROM nodes n WHERE n.id = ?1", no_wake_source()),
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
