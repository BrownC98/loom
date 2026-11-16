use rusqlite::{Connection, OptionalExtension, params};

use api::graph::errors::Refusal;
use api::ids::WaitId;

use super::Graph;

pub struct SentImage {
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy)]
pub enum ImagePart {
    Report,
    Answer,
}

impl ImagePart {
    fn key(self) -> &'static str {
        match self {
            ImagePart::Report => "report",
            ImagePart::Answer => "answer",
        }
    }
}

pub(super) fn put(
    db: &Connection,
    wait: WaitId,
    part: ImagePart,
    images: &[SentImage],
) -> rusqlite::Result<()> {
    for (idx, image) in images.iter().enumerate() {
        db.execute(
            "INSERT OR REPLACE INTO kept_images (wait, part, idx, name, bytes) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![wait.0, part.key(), idx as i64, image.name, image.bytes],
        )?;
    }
    Ok(())
}

pub(super) fn names(
    db: &Connection,
    wait: WaitId,
    part: ImagePart,
) -> rusqlite::Result<Vec<String>> {
    db.prepare_cached("SELECT name FROM kept_images WHERE wait = ?1 AND part = ?2 ORDER BY idx")?
        .query_map(params![wait.0, part.key()], |row| row.get(0))?
        .collect()
}

pub(super) fn prune(db: &Connection) -> rusqlite::Result<()> {
    db.execute(
        "DELETE FROM kept_images WHERE wait NOT IN (SELECT id FROM dones)
         AND wait NOT IN (SELECT wait ->> 'id' FROM closed_waits)",
        [],
    )?;
    Ok(())
}

impl Graph {
    pub fn unkept_images(&self) -> rusqlite::Result<Vec<(WaitId, Vec<String>)>> {
        let db = self.lock();
        let mut stmt = db.prepare(
            "SELECT w.wait ->> 'id', w.wait ->> 'images' FROM
               (SELECT wait FROM dones UNION ALL SELECT wait FROM closed_waits) w
             WHERE json_valid(w.wait) AND json_array_length(w.wait, '$.images') > 0
               AND NOT EXISTS (SELECT 1 FROM kept_images k
                 WHERE k.wait = w.wait ->> 'id' AND k.part = ?1)",
        )?;
        stmt.query_map([ImagePart::Report.key()], |row| {
            let paths: String = row.get(1)?;
            Ok((
                WaitId(row.get(0)?),
                serde_json::from_str(&paths).unwrap_or_default(),
            ))
        })?
        .collect()
    }

    pub fn keep_old_images(&self, wait: WaitId, sent: &[SentImage]) -> rusqlite::Result<()> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        put(&tx, wait, ImagePart::Report, sent)?;
        let names: Vec<&str> = sent.iter().map(|i| i.name.as_str()).collect();
        tx.execute(
            "UPDATE dones SET wait = json_set(wait, '$.images', json(?2)) WHERE id = ?1",
            params![wait.0, serde_json::to_string(&names).unwrap_or_default()],
        )?;
        tx.commit()
    }

    pub fn sent_image(
        &self,
        wait: WaitId,
        part: ImagePart,
        index: usize,
    ) -> Result<SentImage, Refusal> {
        self.lock()
            .query_row(
                "SELECT name, bytes FROM kept_images WHERE wait = ?1 AND part = ?2 AND idx = ?3",
                params![wait.0, part.key(), index as i64],
                |row| {
                    Ok(SentImage {
                        name: row.get(0)?,
                        bytes: row.get(1)?,
                    })
                },
            )
            .optional()?
            .ok_or(Refusal::NoWait(wait))
    }
}
