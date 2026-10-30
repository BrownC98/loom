use rusqlite::{Connection, OptionalExtension, params};

use super::waits::unreported_root;
use super::{Graph, NodeId, TurnCheck};
use crate::protocol::kind_line;

#[derive(Clone, Copy)]
enum Bit {
    Human,
    Called,
    KindNudged,
    Asked,
    QuestionBox,
    Replied,
    SubmittedInput,
}

impl Bit {
    const fn mask(self) -> u8 {
        1 << self as u8
    }
}

const _: () = match Bit::SubmittedInput {
    Bit::Human | Bit::Called | Bit::KindNudged | Bit::Asked | Bit::QuestionBox | Bit::Replied => {}
    last @ Bit::SubmittedInput => assert!((last as u32) < u8::BITS),
};

#[derive(Clone, Copy)]
pub enum TurnMark {
    Human,
    Called,
    Asked,
    QuestionBox,
}

impl TurnMark {
    fn mask(self) -> u8 {
        match self {
            TurnMark::Human => Bit::Human,
            TurnMark::Called => Bit::Called,
            TurnMark::Asked => Bit::Asked,
            TurnMark::QuestionBox => Bit::QuestionBox,
        }
        .mask()
    }
}

const REPLIED: u8 = Bit::Replied.mask();
pub(super) const SUBMITTED_INPUT: u8 = Bit::SubmittedInput.mask();

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
    pub fn drafted(&self, id: NodeId) -> rusqlite::Result<bool> {
        self.lock()
            .query_row(
                "SELECT drafted = 1 FROM nodes WHERE id = ?1",
                [id.0],
                |row| row.get(0),
            )
            .optional()
            .map(|drafted| drafted.unwrap_or(false))
    }

    pub fn begin_draft(&self, id: NodeId) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE nodes SET drafted = 1 WHERE id = ?1 AND drafted = 0",
            [id.0],
        )?;
        Ok(())
    }

    pub fn end_draft(&self, id: NodeId) -> rusqlite::Result<bool> {
        Ok(self.lock().execute(
            "UPDATE nodes SET drafted = 0, turn_flags = turn_flags & ~?2
             WHERE id = ?1 AND (drafted = 1 OR (turn_flags & ?2) != 0)",
            params![id.0, SUBMITTED_INPUT],
        )? > 0)
    }

    pub fn mark_turn(&self, node: NodeId, mark: TurnMark) -> rusqlite::Result<()> {
        set_flags(&self.lock(), node, mark.mask(), 0)
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
            self.publish_nodes();
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

    pub fn turn_was_ended(&self, node: NodeId, turn: &str) -> rusqlite::Result<bool> {
        self.lock().query_row(
            "SELECT EXISTS (SELECT 1 FROM ended_turns WHERE node = ?1 AND turn = ?2)",
            params![node.0, turn],
            |row| row.get(0),
        )
    }

    pub fn end_turn(&self, node: NodeId) -> rusqlite::Result<Option<TurnCheck>> {
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
        let has = |bit: Bit| flags & bit.mask() != 0;
        let reply = reply.filter(|_| has(Bit::Replied));
        let check = if has(Bit::QuestionBox) && !has(Bit::Asked) {
            Some(TurnCheck::QuestionBox)
        } else if !has(Bit::Called)
            && !has(Bit::KindNudged)
            && reply
                .as_deref()
                .is_some_and(|text| kind_line(text).is_some())
        {
            Some(TurnCheck::KindLine)
        } else if has(Bit::Human) && reply.is_some() && unreported_root(&tx, node)? {
            Some(TurnCheck::Unreported)
        } else {
            None
        };
        tx.execute(
            "INSERT OR IGNORE INTO ended_turns (node, turn)
             SELECT id, turn FROM nodes WHERE id = ?1 AND turn IS NOT NULL",
            [node.0],
        )?;
        let kept = if check == Some(TurnCheck::KindLine) {
            Bit::KindNudged.mask()
        } else {
            0
        };
        tx.execute(
            "UPDATE nodes SET turn_flags = ?2, turn = NULL WHERE id = ?1",
            params![node.0, kept | (flags & SUBMITTED_INPUT)],
        )?;
        tx.commit()?;
        Ok(check)
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
            TurnMark::Asked.mask() | TurnMark::QuestionBox.mask() | REPLIED,
        )?;
        tx.commit()
    }
}
