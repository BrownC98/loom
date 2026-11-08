use rusqlite::{Connection, OptionalExtension, params};

use super::Graph;
use super::gate::{Unjudged, unjudged};
use super::waits::{put_done, unreported_root};
use crate::protocol::kind_line;
use api::graph::events::TurnCheck;
use api::ids::NodeId;

#[derive(Clone, Copy)]
enum Bit {
    Human,
    Called,
    KindNudged,
    Asked,
    QuestionBox,
    Replied,
    SubmittedInput,
    FirstInput,
    UnjudgedNudged,
    Status,
    Typed,
    HumanReply,
    Background,
}

impl Bit {
    const fn mask(self) -> u16 {
        1 << self as u16
    }
}

const _: () = match Bit::Background {
    Bit::Human
    | Bit::Called
    | Bit::KindNudged
    | Bit::Asked
    | Bit::QuestionBox
    | Bit::Replied
    | Bit::SubmittedInput
    | Bit::FirstInput
    | Bit::UnjudgedNudged
    | Bit::Status
    | Bit::Typed
    | Bit::HumanReply => {}
    last @ Bit::Background => assert!((last as u32) < u16::BITS),
};

const NUDGED: u16 = Bit::KindNudged.mask() | Bit::UnjudgedNudged.mask();

pub enum Nudge {
    QuestionBox,
    KindLine,
    Unjudged(Unjudged),
}

impl Nudge {
    pub fn check(&self) -> TurnCheck {
        match self {
            Nudge::QuestionBox => TurnCheck::QuestionBox,
            Nudge::KindLine => TurnCheck::KindLine,
            Nudge::Unjudged(_) => TurnCheck::Unjudged,
        }
    }

    fn kept(&self) -> u16 {
        match self {
            Nudge::QuestionBox => 0,
            Nudge::KindLine => Bit::KindNudged.mask(),
            Nudge::Unjudged(_) => Bit::UnjudgedNudged.mask(),
        }
    }
}

#[derive(Clone, Copy)]
pub enum TurnMark {
    Called,
    Asked,
    QuestionBox,
}

impl TurnMark {
    fn mask(self) -> u16 {
        match self {
            TurnMark::Called => Bit::Called,
            TurnMark::Asked => Bit::Asked,
            TurnMark::QuestionBox => Bit::QuestionBox,
        }
        .mask()
    }
}

const REPLIED: u16 = Bit::Replied.mask();
pub(super) const STATUS: u16 = Bit::Status.mask();
pub(super) const HUMAN_REPLY: u16 = Bit::HumanReply.mask();
pub(super) const TYPED: u16 = Bit::Typed.mask();
pub(super) const BACKGROUND: u16 = Bit::Background.mask();
pub(super) const SUBMITTED_INPUT: u16 = Bit::SubmittedInput.mask();
const FIRST_INPUT: u16 = Bit::FirstInput.mask();

fn set_flags(db: &Connection, node: NodeId, set: u16, clear: u16) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE nodes SET turn_flags = (turn_flags & ~?3) | ?2 WHERE id = ?1",
        params![node.0, set, clear],
    )
    .map(drop)
}

fn keep_reply(db: &Connection, node: NodeId, text: &str) -> rusqlite::Result<bool> {
    Ok(!text.is_empty()
        && db.execute(
            "UPDATE nodes SET reply = ?2 WHERE id = ?1 AND reply IS NOT ?2",
            params![node.0, text],
        )? > 0)
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

    pub fn heard(&self, node: NodeId, said: Option<&str>) -> rusqlite::Result<()> {
        self.lock()
            .execute(
                "UPDATE nodes SET turn_flags = turn_flags | ?2, heard = ?3 WHERE id = ?1",
                params![node.0, Bit::Human.mask(), said],
            )
            .map(drop)
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
            set_flags(&tx, node, 0, REPLIED | STATUS)?;
            return tx.commit();
        }
        set_flags(&tx, node, REPLIED, STATUS)?;
        let changed = keep_reply(&tx, node, text)?;
        tx.commit()?;
        if changed {
            self.publish_nodes();
        }
        Ok(())
    }

    pub fn status(&self, node: NodeId, text: &str) -> rusqlite::Result<()> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        set_flags(&tx, node, STATUS, REPLIED)?;
        let changed = keep_reply(&tx, node, text.trim())?;
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

    pub fn end_turn(&self, node: NodeId) -> rusqlite::Result<Option<Nudge>> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let (flags, reply, heard): (u16, Option<String>, Option<String>) = tx
            .query_row(
                "SELECT turn_flags, reply, heard FROM nodes WHERE id = ?1",
                [node.0],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?
            .unwrap_or_default();
        let has = |bit: Bit| flags & bit.mask() != 0;
        let reply = reply.filter(|_| has(Bit::Replied));
        let check = if has(Bit::QuestionBox) && !has(Bit::Asked) {
            Some(Nudge::QuestionBox)
        } else if !has(Bit::Called)
            && !has(Bit::KindNudged)
            && reply
                .as_deref()
                .is_some_and(|text| kind_line(text).is_some())
        {
            Some(Nudge::KindLine)
        } else if !has(Bit::UnjudgedNudged) {
            unjudged(&tx, node)?.map(Nudge::Unjudged)
        } else {
            None
        };
        let reported = match reply.as_deref() {
            Some(reply)
                if check.is_none()
                    && has(Bit::Human)
                    && !has(Bit::Called)
                    && unreported_root(&tx, node)? =>
            {
                put_done(
                    &tx,
                    node,
                    heard.as_deref().unwrap_or_default(),
                    reply,
                    &[],
                    None,
                )?;
                true
            }
            _ => false,
        };
        tx.execute(
            "INSERT OR IGNORE INTO ended_turns (node, turn)
             SELECT id, turn FROM nodes WHERE id = ?1 AND turn IS NOT NULL",
            [node.0],
        )?;
        let kept = check
            .as_ref()
            .map_or(0, |nudge| (flags & NUDGED) | nudge.kept());
        let open = if has(Bit::Status) && !has(Bit::Called) {
            Bit::Human.mask()
        } else {
            0
        };
        let human_reply = if has(Bit::Typed) && !has(Bit::Status) {
            HUMAN_REPLY
        } else {
            0
        };
        tx.execute(
            "UPDATE nodes SET turn_flags = ?2, turn = NULL WHERE id = ?1",
            params![
                node.0,
                kept | human_reply | (flags & (SUBMITTED_INPUT | STATUS | BACKGROUND | open))
            ],
        )?;
        tx.commit()?;
        if reported {
            self.publish_nodes();
        }
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
            TurnMark::Asked.mask()
                | TurnMark::QuestionBox.mask()
                | REPLIED
                | STATUS
                | HUMAN_REPLY
                | TYPED
                | BACKGROUND
                | FIRST_INPUT,
        )?;
        tx.commit()
    }

    pub fn background(&self, node: NodeId, running: bool) -> rusqlite::Result<()> {
        let (set, clear) = if running {
            (BACKGROUND, 0)
        } else {
            (0, BACKGROUND)
        };
        set_flags(&self.lock(), node, set, clear)
    }

    pub fn first_input_sent(&self, node: NodeId) -> rusqlite::Result<()> {
        set_flags(&self.lock(), node, FIRST_INPUT, 0)
    }

    pub fn first_input_pending(&self, node: NodeId) -> rusqlite::Result<bool> {
        self.lock()
            .query_row(
                "SELECT (turn_flags & ?2) != 0 FROM nodes WHERE id = ?1",
                params![node.0, FIRST_INPUT],
                |row| row.get(0),
            )
            .optional()
            .map(|pending| pending.unwrap_or(false))
    }

    pub fn first_input_taken(&self, node: NodeId) -> rusqlite::Result<bool> {
        Ok(self.lock().execute(
            "UPDATE nodes SET turn_flags = turn_flags & ~?2 WHERE id = ?1 AND (turn_flags & ?2) != 0",
            params![node.0, FIRST_INPUT],
        )? > 0)
    }
}
