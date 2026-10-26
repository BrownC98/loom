use std::sync::Mutex;
use std::time::{Duration, Instant};

use rusqlite::params;
use serde::Serialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::watch;
use ts_rs::TS;

use crate::db::Database;
use crate::pake;

pub const INVITE_TTL: Duration = Duration::from_secs(10 * 60);
const INVITE_TRIES: u8 = 5;
const CODE_LEN: usize = 8;
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTWXYZ23456789";
const SEEN_EVERY_MS: i64 = 60_000;
const LABEL_MAX: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, TS)]
#[ts(export)]
pub struct DeviceId(#[ts(type = "number")] pub i64);

#[derive(Clone, PartialEq, Eq, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeviceView {
    pub id: DeviceId,
    pub label: String,
    /// Unix 밀리초.
    #[ts(type = "number")]
    pub paired_at: i64,
    #[ts(type = "number")]
    pub last_seen: i64,
}

pub struct DeviceToken(pub String);

#[derive(Debug, PartialEq, Eq)]
pub enum PairRefused {
    Wrong,
}

struct Invite {
    code: String,
    until: Instant,
    tries: u8,
    asked: Vec<[u8; 32]>,
}

pub struct Devices {
    db: Database,
    invite: Mutex<Option<Invite>>,
    removed: watch::Sender<u64>,
}

impl Devices {
    pub fn new(db: Database) -> Self {
        Self {
            db,
            invite: Mutex::default(),
            removed: watch::Sender::new(0),
        }
    }

    pub fn invite(&self) -> String {
        let code = random_code();
        *self.invite.lock().unwrap() = Some(Invite {
            code: code.clone(),
            until: Instant::now() + INVITE_TTL,
            tries: 0,
            asked: Vec::new(),
        });
        code
    }

    pub fn invite_open(&self) -> bool {
        let mut invite = self.invite.lock().unwrap();
        if invite.as_ref().is_some_and(|i| i.until <= Instant::now()) {
            *invite = None;
        }
        invite.is_some()
    }

    pub fn pair(&self, code: &str, label: &str) -> Result<DeviceToken, PairRefused> {
        let code: String = code
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_uppercase())
            .collect();
        {
            let mut slot = self.invite.lock().unwrap();
            let Some(invite) = slot.as_mut() else {
                return Err(PairRefused::Wrong);
            };
            if invite.until <= Instant::now() {
                *slot = None;
                return Err(PairRefused::Wrong);
            }
            if !bool::from(code.as_bytes().ct_eq(invite.code.as_bytes())) {
                invite.tries += 1;
                if invite.tries >= INVITE_TRIES {
                    *slot = None;
                }
                return Err(PairRefused::Wrong);
            }
            *slot = None;
        }
        let token = pake::random_token();
        let now = now_ms();
        let label: String = label.chars().take(LABEL_MAX).collect();
        self.db
            .lock()
            .execute(
                "INSERT INTO devices (label, token_hash, paired_at, last_seen) VALUES (?1, ?2, ?3, ?3)",
                params![label, hash(&token).as_slice(), now],
            )
            .expect("devices 표는 늘 있다");
        Ok(DeviceToken(token))
    }

    pub fn answer(&self, pa: &pake::Element, door: &[u8]) -> Option<pake::Answer> {
        let code = {
            let mut slot = self.invite.lock().unwrap();
            let invite = slot.as_mut()?;
            let digest = pa.digest();
            if invite.until <= Instant::now()
                || !invite.asked.contains(&digest) && invite.tries >= INVITE_TRIES
            {
                *slot = None;
                return None;
            }
            if !invite.asked.contains(&digest) {
                invite.tries += 1;
                invite.asked.push(digest);
            }
            invite.code.clone()
        };
        Some(pake::answer(&code, pa, door))
    }

    pub fn check(&self, token: &str) -> Option<DeviceId> {
        let wanted = hash(token);
        let db = self.db.lock();
        let mut rows = db
            .prepare_cached("SELECT id, token_hash FROM devices")
            .ok()?;
        let all: Vec<(i64, Vec<u8>)> = rows
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .ok()?
            .filter_map(Result::ok)
            .collect();
        let mut found = None;
        for (id, stored) in all {
            if bool::from(stored.as_slice().ct_eq(wanted.as_slice())) {
                found = Some(id);
            }
        }
        let id = found?;
        let now = now_ms();
        let _ = db.execute(
            "UPDATE devices SET last_seen = ?1 WHERE id = ?2 AND last_seen < ?1 - ?3",
            params![now, id, SEEN_EVERY_MS],
        );
        Some(DeviceId(id))
    }

    pub fn contains(&self, id: DeviceId) -> bool {
        self.db
            .lock()
            .query_row("SELECT 1 FROM devices WHERE id = ?1", [id.0], |_| Ok(()))
            .is_ok()
    }

    pub fn list(&self) -> rusqlite::Result<Vec<DeviceView>> {
        let db = self.db.lock();
        let mut rows = db.prepare(
            "SELECT id, label, paired_at, last_seen FROM devices ORDER BY paired_at DESC",
        )?;
        rows.query_map([], |row| {
            Ok(DeviceView {
                id: DeviceId(row.get(0)?),
                label: row.get(1)?,
                paired_at: row.get(2)?,
                last_seen: row.get(3)?,
            })
        })?
        .collect()
    }

    pub fn remove(&self, id: DeviceId) -> rusqlite::Result<bool> {
        let removed = self
            .db
            .lock()
            .execute("DELETE FROM devices WHERE id = ?1", [id.0])?
            > 0;
        if removed {
            self.removed.send_modify(|n| *n += 1);
        }
        Ok(removed)
    }

    pub async fn removed(&self, id: DeviceId) {
        let mut changes = self.removed.subscribe();
        while self.contains(id) {
            if changes.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }
}

fn hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

fn random_code() -> String {
    let limit = (256 / CODE_ALPHABET.len() * CODE_ALPHABET.len()) as u8;
    let mut code = String::with_capacity(CODE_LEN);
    while code.len() < CODE_LEN {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).expect("OS 난수를 읽지 못했다");
        for b in bytes.into_iter().filter(|&b| b < limit) {
            if code.len() < CODE_LEN {
                code.push(CODE_ALPHABET[b as usize % CODE_ALPHABET.len()] as char);
            }
        }
    }
    code
}

fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}
