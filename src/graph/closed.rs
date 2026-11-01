use rusqlite::{Connection, Row, params};

pub use api::graph::waits::{Closed, ClosedPage, ClosedWait};

use super::events::Who;
use super::waits::HumanWait;
use super::{Graph, NodeId, from_json, name_of, to_json};
use crate::protocol::WaitId;

const KEEP_SECS: i64 = 30 * 24 * 60 * 60;
const PAGE: u32 = 100;

#[derive(Clone, Copy)]
pub(super) enum Open {
    Question,
    Done,
}

impl Open {
    fn table(self) -> &'static str {
        match self {
            Open::Question => "questions",
            Open::Done => "dones",
        }
    }
}

pub(super) fn close(
    db: &Connection,
    open: Open,
    wait: WaitId,
    how: &Closed,
) -> rusqlite::Result<Option<NodeId>> {
    let mut stmt = db.prepare(&format!(
        "DELETE FROM {} WHERE id = ?1 RETURNING node, wait",
        open.table()
    ))?;
    let rows = stmt
        .query_map([wait.0], open_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    keep(db, &rows, how)?;
    Ok(rows.first().map(|(node, _)| *node))
}

pub(super) fn close_all(
    db: &Connection,
    open: Open,
    node: NodeId,
    how: &Closed,
) -> rusqlite::Result<Vec<WaitId>> {
    let mut stmt = db.prepare(&format!(
        "DELETE FROM {} WHERE node = ?1 RETURNING node, wait",
        open.table()
    ))?;
    let rows = stmt
        .query_map([node.0], open_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    keep(db, &rows, how)?;
    Ok(rows
        .iter()
        .filter_map(|(_, wait)| serde_json::from_str::<HumanWait>(wait).ok())
        .map(|w| w.id)
        .collect())
}

fn open_row(row: &Row) -> rusqlite::Result<(NodeId, String)> {
    Ok((NodeId(row.get(0)?), row.get(1)?))
}

fn keep(db: &Connection, rows: &[(NodeId, String)], how: &Closed) -> rusqlite::Result<()> {
    for (node, wait) in rows {
        db.execute(
            "INSERT INTO closed_waits (node, name, wait, closed_at, how) VALUES (?1, ?2, ?3, unixepoch(), ?4)",
            params![node.0, name_of(db, *node)?, wait, to_json(how)],
        )?;
    }
    prune(db)
}

fn prune(db: &Connection) -> rusqlite::Result<()> {
    db.execute(
        "DELETE FROM closed_waits WHERE closed_at < unixepoch() - ?1",
        [KEEP_SECS],
    )?;
    Ok(())
}

impl Graph {
    pub fn closed_waits(&self, before: Option<u32>) -> rusqlite::Result<ClosedPage> {
        let db = self.lock();
        prune(&db)?;
        let mut waits = db
            .prepare(
                "SELECT c.seq, c.node, c.name, c.wait, c.closed_at, c.how,
                    EXISTS(SELECT 1 FROM nodes n WHERE n.id = c.node)
                 FROM closed_waits c WHERE c.seq < ?1 ORDER BY c.seq DESC LIMIT ?2",
            )?
            .query_map(
                params![before.map_or(i64::MAX, i64::from), PAGE + 1],
                closed_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let more = waits.len() > PAGE as usize;
        waits.truncate(PAGE as usize);
        Ok(ClosedPage { waits, more })
    }
}

fn closed_row(row: &Row) -> rusqlite::Result<ClosedWait> {
    let wait = super::waits::for_screen(from_json::<HumanWait>(row, 3)?);
    let how: Closed = from_json(row, 5)?;
    let chosen = match &how {
        Closed::Answered { text } => wait
            .wait
            .choices
            .iter()
            .find(|c| text.trim().eq_ignore_ascii_case(&c.key.to_string()))
            .map(|c| c.key),
        _ => None,
    };
    let alive: bool = row.get(6)?;
    Ok(ClosedWait {
        seq: row.get(0)?,
        wait,
        node: Who {
            id: alive.then_some(NodeId(row.get(1)?)),
            name: row.get(2)?,
        },
        closed_at: row.get(4)?,
        how,
        chosen,
    })
}
