use rusqlite::{Connection, Row, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::events::{ClosedBy, Who};
use super::waits::{HumanWait, ScreenWait};
use super::{Graph, NodeId, from_json, name_of, to_json};
use crate::protocol::WaitId;

const KEEP_SECS: i64 = 30 * 24 * 60 * 60;
const PAGE: u32 = 100;

/// 알림이 닫힌 방법.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Closed {
    /// 사람이 받은편지함에서 질문에 답했다.
    Answered {
        text: String,
    },
    /// 사람이 완료 줄에서 이어서 맡겼다.
    Instructed {
        text: String,
    },
    /// 사람이 완료를 [확인]했다.
    Confirmed,
    Unanswered {
        by: ClosedBy,
    },
    /// 알림이 열린 채 노드를 지웠다.
    NodeRemoved,
}

/// 「기록」 탭의 한 줄: 닫힌 질문이나 완료.
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClosedWait {
    /// 닫힌 차례. 다음 쪽은 이 값보다 작은 줄이다.
    pub seq: u32,
    pub wait: ScreenWait,
    /// 노드를 지웠으면 `id`가 없다.
    pub node: Who,
    #[ts(type = "number")]
    pub closed_at: i64,
    pub how: Closed,
    /// 답이 선택지 하나의 `key`와 같으면 그 `key`다.
    pub chosen: Option<char>,
}

/// 닫힌 알림 한 쪽. 새것부터.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct ClosedPage {
    pub waits: Vec<ClosedWait>,
    pub more: bool,
}

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
    db.execute(
        "DELETE FROM closed_waits WHERE closed_at < unixepoch() - ?1",
        [KEEP_SECS],
    )?;
    Ok(())
}

impl Graph {
    pub fn closed_waits(&self, before: Option<u32>) -> rusqlite::Result<ClosedPage> {
        let db = self.lock();
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
    let wait = from_json::<HumanWait>(row, 3)?.for_screen();
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
