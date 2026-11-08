use rusqlite::{Connection, Row, params};

use api::graph::events::{Entry, EventLog, EventRecord, FieldParts, MessageFacts, Who};

use super::{from_json, to_json};
use crate::choices::choices;
use crate::link_text::{parts, path_link};
use crate::protocol::MessageId;
use api::graph::messages::{Field, MessageKind};
use api::graph::text::TextPart;
use api::ids::NodeId;

const KEEP: u32 = 10_000;
const RECENT: u32 = 500;

fn record(
    id: u32,
    at: i64,
    entry: Entry,
    message: Option<u32>,
    body: Option<String>,
) -> EventRecord {
    let text = match &entry {
        Entry::Message { what, .. } => body
            .as_deref()
            .filter(|body| !what.as_ref().is_some_and(|w| w.repeats_file(body))),
        Entry::Asked { text, .. }
        | Entry::Answered { text, .. }
        | Entry::Instructed { text, .. } => Some(text.as_str()),
        Entry::Reported { summary, .. } => Some(summary.as_str()),
        _ => None,
    };
    let question = match &entry {
        Entry::Message {
            what: Some(MessageKind::Ask),
            ..
        } => text,
        _ => None,
    };
    let message = match (&entry, message) {
        (Entry::Message { what, .. }, Some(id)) => Some(MessageFacts {
            id,
            from_loom: what.as_ref().is_some_and(shown_from_loom),
            fields: screen_fields(what.as_ref()),
        }),
        _ => None,
    };
    EventRecord {
        id,
        at,
        parts: text.map(parts).unwrap_or_default(),
        choices: question.map(choices).unwrap_or_default(),
        message,
        entry,
        body,
    }
}

fn screen_fields(what: Option<&MessageKind>) -> Vec<FieldParts> {
    let mut fields: Vec<FieldParts> = Vec::new();
    for (name, value) in what.iter().flat_map(|w| w.fields()) {
        match (name, fields.last_mut()) {
            (Field::Judgment, _) => {}
            (Field::Head, Some(branch)) if branch.name == Field::Branch => {
                branch.parts.push(TextPart::Text {
                    text: format!(" · {value}"),
                });
            }
            _ => fields.push(FieldParts {
                name,
                parts: if name.is_file() {
                    vec![path_link(&value, &crate::task::ROOT)]
                } else {
                    parts(&value)
                },
            }),
        }
    }
    fields
}

fn shown_from_loom(what: &MessageKind) -> bool {
    match what {
        MessageKind::Merged { .. }
        | MessageKind::Withdrawn { .. }
        | MessageKind::Rework { .. }
        | MessageKind::Failed { .. }
        | MessageKind::Exited { .. }
        | MessageKind::SetupFailed { .. }
        | MessageKind::Removed { .. }
        | MessageKind::Adopted { .. }
        | MessageKind::Idle
        | MessageKind::RulesChanged { .. } => true,
        MessageKind::Ask
        | MessageKind::Answer { .. }
        | MessageKind::Done { .. }
        | MessageKind::Note
        | MessageKind::Ready { .. }
        | MessageKind::Verdict { .. }
        | MessageKind::HumanInput { .. } => false,
    }
}

const RECORD_COLUMNS: &str = "SELECT e.id, e.at, e.entry, m.body, e.message FROM events e
    LEFT JOIN messages m ON m.id = e.message";

pub(super) fn write(
    db: &Connection,
    entry: &Entry,
    message: Option<MessageId>,
) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO events (at, entry, message) VALUES (unixepoch(), ?1, ?2)",
        params![to_json(entry), message.map(|m| m.0)],
    )?;
    db.execute(
        "DELETE FROM events WHERE id <= ?1",
        [db.last_insert_rowid() - KEEP as i64],
    )?;
    Ok(())
}

pub(super) fn oldest_at(db: &Connection) -> rusqlite::Result<Option<i64>> {
    db.query_row("SELECT min(at) FROM events", [], |row| row.get(0))
}

pub(super) fn sent_since(db: &Connection, since: i64) -> rusqlite::Result<Vec<EventRecord>> {
    db.prepare(&format!(
        "{RECORD_COLUMNS} WHERE e.at >= ?1 AND CASE WHEN json_valid(e.entry) THEN e.entry ->> 'kind' = 'message' AND e.entry ->> 'step' = 'sent' END ORDER BY e.id"
    ))?
    .query_map([since], record_from_row)?
    .collect()
}

pub(super) fn who(db: &Connection, id: NodeId) -> rusqlite::Result<Who> {
    Ok(Who {
        id: Some(id),
        name: super::name_of(db, id)?,
    })
}

pub(super) fn log(db: &Connection) -> rusqlite::Result<EventLog> {
    let records = db
        .prepare(&format!(
            "{RECORD_COLUMNS} WHERE CASE WHEN json_valid(e.entry) THEN e.entry ->> 'kind' NOT IN ('idleOpened', 'idleEnded', 'endpointFailed', 'agentStateChanged', 'agentStateDisagreed') END ORDER BY e.id DESC LIMIT ?1"
        ))?
        .query_map([RECENT], record_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(EventLog {
        last: last_id(db)?,
        records,
    })
}

pub(super) fn after(db: &Connection, after: u32) -> rusqlite::Result<Vec<EventRecord>> {
    db.prepare(&format!(
        "{RECORD_COLUMNS} WHERE e.id > ?1 AND json_valid(e.entry) ORDER BY e.id"
    ))?
    .query_map([after], record_from_row)?
    .collect()
}

pub(super) fn last_id(db: &Connection) -> rusqlite::Result<u32> {
    let id: Option<u32> = db.query_row("SELECT max(id) FROM events", [], |row| row.get(0))?;
    Ok(id.unwrap_or(0))
}

fn record_from_row(row: &Row) -> rusqlite::Result<EventRecord> {
    Ok(record(
        row.get(0)?,
        row.get(1)?,
        from_json(row, 2)?,
        row.get(4)?,
        row.get(3)?,
    ))
}
