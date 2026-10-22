//! 짝지은 폰과 짝짓기 코드. 폰은 PC 화면이 낸 코드를 한 번 보내고 기기 토큰(쿠키)을 받는다.
//! 근거: docs/design.md 「원격」, 목업 docs/mockups/phone-pairing.html(2026-10-22 사람 결정 Q364 A, Orca 「휴대폰 페어링」을 따름).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine;
use rusqlite::params;
use serde::Serialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::watch;
use ts_rs::TS;

use crate::db::Database;
use crate::pake;

/// 코드가 사는 시간. Orca 짝짓기 코드와 같다(`MAX_INVITE_TTL_MS` 10분).
pub const INVITE_TTL: Duration = Duration::from_secs(10 * 60);
/// 한 코드로 맞혀 볼 수 있는 횟수: 틀린 `pair`와 처음 보는 `pA`(`answer`)를 함께 센다. 넘으면 코드를 지운다.
/// 8자(29글자 표)는 약 39비트라 5번으로는 맞힐 수 없다.
const INVITE_TRIES: u8 = 5;
const CODE_LEN: usize = 8;
/// 헷갈리는 글자(0·O, 1·I·L, U·V)를 뺐다. 사람이 폰에 옮겨 적는다.
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTWXYZ23456789";
/// `last_seen`을 이 간격보다 자주 쓰지 않는다. 화면은 요청마다 기기를 확인한다.
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

/// 쿠키에 넣는 비밀. DB에는 해시만 둔다.
pub struct DeviceToken(pub String);

#[derive(Debug, PartialEq, Eq)]
pub enum PairRefused {
    /// 코드가 없거나, 시간이 지났거나, 틀렸다. 폰에는 같은 글을 보인다.
    Wrong,
}

struct Invite {
    code: String,
    until: Instant,
    tries: u8,
    /// 답한 `pA`의 SHA-256. 폰이 같은 `pA`를 다시 보내면(UDP를 잃었을 때) 다시 세지 않는다.
    asked: Vec<[u8; 32]>,
}

pub struct Devices {
    db: Database,
    /// 열린 코드는 하나다. 새 코드를 만들면 앞 코드는 쓸 수 없다.
    invite: Mutex<Option<Invite>>,
    /// 기기를 지울 때마다 바뀐다. 열린 웹소켓이 자기 기기가 남았는지 다시 본다.
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

    /// 새 코드를 만든다. 앞 코드는 버린다.
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

    /// 쓸 수 있는 코드가 있는지. 화면이 코드 상자를 닫을 때를 안다(폰이 썼거나 시간이 지났다).
    pub fn invite_open(&self) -> bool {
        let mut invite = self.invite.lock().unwrap();
        if invite.as_ref().is_some_and(|i| i.until <= Instant::now()) {
            *invite = None;
        }
        invite.is_some()
    }

    /// 코드가 맞으면 기기를 더하고 토큰을 준다. 코드 확인과 지우기를 한 잠금 안에서 한다. 그래서 같은 코드로 두 기기가 짝짓지 못한다.
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
        let token = random_token();
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

    /// 같은 Wi-Fi의 폰이 코드로 PC를 찾을 때 보낸 `pA`에 답한다(`pake`). 열린 코드가 없으면 답하지 않는다.
    /// 처음 보는 `pA`는 한 번 맞혀 보기로 센다. 다섯 번을 넘기면 코드를 지운다. 맞으면 폰이 이어서 `pair`를 부른다.
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

    /// 토큰의 기기. 해시를 모두 읽어 시간이 같은 비교로 찾는다. 기기는 몇 개뿐이다.
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

    /// 지웠으면 `true`. 그 기기의 다음 요청과 열린 웹소켓을 끊는다.
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

    /// `id`가 지워지면 끝난다. 폰의 웹소켓을 이 미래와 함께 돌려 끊는다.
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

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("OS 난수를 읽지 못했다");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// 고른 글자 수의 배수 아래 바이트만 써서 글자마다 확률이 같다.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn devices() -> (tempfile::TempDir, Devices) {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::open(&dir.path().join("t.db")).unwrap();
        (dir, Devices::new(db))
    }

    #[test]
    fn code_pairs_once_and_token_checks() {
        let (_dir, d) = devices();
        let code = d.invite();
        assert_eq!(code.len(), CODE_LEN);
        assert!(code.bytes().all(|b| CODE_ALPHABET.contains(&b)));
        assert!(d.invite_open());
        // 사람이 적는 꼴(소문자, 가운데 줄표)도 받는다.
        let typed = format!("{}-{}", &code[..4], &code[4..]).to_lowercase();
        let token = d.pair(&typed, "iPhone · Safari").unwrap();
        assert!(!d.invite_open());
        assert!(matches!(d.pair(&code, "x"), Err(PairRefused::Wrong)));
        let id = d.check(&token.0).unwrap();
        assert!(d.check("not-a-token").is_none());
        assert!(d.check("").is_none());
        let list = d.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].label, "iPhone · Safari");
        assert!(d.remove(id).unwrap());
        assert!(d.check(&token.0).is_none());
        assert!(!d.remove(id).unwrap());
    }

    #[test]
    fn tokens_are_stored_as_hashes() {
        let (_dir, d) = devices();
        let code = d.invite();
        let token = d.pair(&code, "a").unwrap();
        let stored: Vec<u8> =
            d.db.lock()
                .query_row("SELECT token_hash FROM devices", [], |r| r.get(0))
                .unwrap();
        assert_eq!(stored, hash(&token.0));
        assert_ne!(stored, token.0.as_bytes());
    }

    #[test]
    fn wrong_codes_burn_the_invite() {
        let (_dir, d) = devices();
        let code = d.invite();
        for _ in 0..INVITE_TRIES {
            assert!(d.pair("WRONGXXX", "a").is_err());
        }
        assert!(!d.invite_open());
        assert!(d.pair(&code, "a").is_err());
        // 다음 코드는 다시 5번이다.
        let code = d.invite();
        for _ in 0..INVITE_TRIES - 1 {
            assert!(d.pair("WRONGXXX", "a").is_err());
        }
        assert!(d.pair(&code, "a").is_ok());
    }

    #[test]
    fn answers_count_as_tries() {
        let (_dir, d) = devices();
        assert!(d.answer(&pake::Element::power(1), b"").is_none());
        let code = d.invite();
        let asks: Vec<_> = (2..2 + u32::from(INVITE_TRIES))
            .map(pake::Element::power)
            .collect();
        for pa in &asks {
            assert!(d.answer(pa, b"").is_some());
        }
        // 같은 `pA`를 다시 보내면 세지 않는다.
        assert!(d.answer(&asks[0], b"").is_some());
        assert!(d.answer(&pake::Element::power(99), b"").is_none());
        assert!(!d.invite_open());
        assert!(d.pair(&code, "a").is_err());
        // 다섯째 답 뒤에도 맞는 코드로 짝짓는다.
        let code = d.invite();
        for pa in &asks {
            assert!(d.answer(pa, b"").is_some());
        }
        assert!(d.pair(&code, "a").is_ok());
    }

    #[test]
    fn new_invite_replaces_old_and_expired_fails() {
        let (_dir, d) = devices();
        let old = d.invite();
        let new = d.invite();
        assert!(d.pair(&old, "a").is_err());
        d.invite.lock().unwrap().as_mut().unwrap().until = Instant::now();
        assert!(d.pair(&new, "a").is_err());
        assert!(!d.invite_open());
        assert!(d.pair("", "a").is_err());
    }

    #[test]
    fn one_code_pairs_one_device_under_race() {
        let (_dir, d) = devices();
        let code = d.invite();
        let d = std::sync::Arc::new(d);
        let wins = std::thread::scope(|s| {
            let handles: Vec<_> = (0..16)
                .map(|_| {
                    let d = d.clone();
                    let code = code.clone();
                    s.spawn(move || d.pair(&code, "a").is_ok())
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .filter(|&ok| ok)
                .count()
        });
        assert_eq!(wins, 1);
        assert_eq!(d.list().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn removed_ends_when_device_goes() {
        let (_dir, d) = devices();
        let code = d.invite();
        let token = d.pair(&code, "a").unwrap();
        let id = d.check(&token.0).unwrap();
        let d = std::sync::Arc::new(d);
        let waiting = tokio::spawn({
            let d = d.clone();
            async move { d.removed(id).await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiting.is_finished());
        d.remove(id).unwrap();
        tokio::time::timeout(Duration::from_secs(2), waiting)
            .await
            .unwrap()
            .unwrap();
    }
}
