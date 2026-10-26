use rusqlite::{Connection, OptionalExtension, params};

use super::{Graph, NodeId};

#[derive(Clone, Copy)]
pub enum TurnMark {
    Human = 1,
    Called = 2,
    KindNudged = 4,
    Asked = 8,
    QuestionBox = 16,
}

const REPLIED: u8 = 32;

pub struct TurnTaken {
    flags: u8,
    pub reply: Option<String>,
}

impl TurnTaken {
    pub fn has(&self, mark: TurnMark) -> bool {
        self.flags & mark as u8 != 0
    }
}

fn set_flags(db: &Connection, node: NodeId, set: u8, clear: u8) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE nodes SET turn_flags = (turn_flags & ~?3) | ?2 WHERE id = ?1",
        params![node.0, set, clear],
    )
    .map(drop)
}

fn late(db: &Connection, node: NodeId, turn: &str) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT (n.turn IS NOT NULL AND n.turn != ?2)
             OR EXISTS (SELECT 1 FROM ended_turns e WHERE e.node = n.id AND e.turn = ?2)
         FROM nodes n WHERE n.id = ?1",
        params![node.0, turn],
        |row| row.get(0),
    )
    .optional()
    .map(|late| late.unwrap_or(true))
}

impl Graph {
    pub fn mark_turn(&self, node: NodeId, mark: TurnMark) -> rusqlite::Result<()> {
        set_flags(&self.lock(), node, mark as u8, 0)
    }

    pub fn said(&self, node: NodeId, text: &str, turn: Option<&str>) -> rusqlite::Result<()> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        if let Some(turn) = turn
            && late(&tx, node, turn)?
        {
            return Ok(());
        }
        let text = text.trim();
        if text.is_empty() {
            set_flags(&tx, node, 0, REPLIED)?;
            return tx.commit();
        }
        set_flags(&tx, node, REPLIED, 0)?;
        let changed = tx.execute(
            "UPDATE nodes SET reply = ?2 WHERE id = ?1 AND reply IS NOT ?2",
            params![node.0, text],
        )? > 0;
        tx.commit()?;
        if changed {
            self.publish_node(&db, node)?;
        }
        Ok(())
    }

    pub fn stepped(&self, node: NodeId, turn: &str) -> rusqlite::Result<()> {
        self.lock()
            .execute(
                "UPDATE nodes SET turn = ?2 WHERE id = ?1",
                params![node.0, turn],
            )
            .map(drop)
    }

    pub fn stopped(&self, node: NodeId, turn: &str) -> rusqlite::Result<()> {
        self.lock()
            .execute(
                "INSERT OR IGNORE INTO ended_turns (node, turn) SELECT id, ?2 FROM nodes WHERE id = ?1",
                params![node.0, turn],
            )
            .map(drop)
    }

    pub fn take_turn(&self, node: NodeId) -> rusqlite::Result<TurnTaken> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let (flags, reply): (u8, Option<String>) = tx
            .query_row(
                "SELECT turn_flags, reply FROM nodes WHERE id = ?1",
                [node.0],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .unwrap_or_default();
        tx.execute(
            "INSERT OR IGNORE INTO ended_turns (node, turn)
             SELECT id, turn FROM nodes WHERE id = ?1 AND turn IS NOT NULL",
            [node.0],
        )?;
        tx.execute(
            "UPDATE nodes SET turn_flags = 0, turn = NULL WHERE id = ?1",
            [node.0],
        )?;
        tx.commit()?;
        Ok(TurnTaken {
            flags,
            reply: reply.filter(|_| flags & REPLIED != 0),
        })
    }

    pub fn turn_launched(&self, node: NodeId) -> rusqlite::Result<()> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        tx.execute("DELETE FROM ended_turns WHERE node = ?1", [node.0])?;
        tx.execute("UPDATE nodes SET turn = NULL WHERE id = ?1", [node.0])?;
        set_flags(
            &tx,
            node,
            0,
            TurnMark::Asked as u8 | TurnMark::QuestionBox as u8 | REPLIED,
        )?;
        tx.commit()
    }
}
