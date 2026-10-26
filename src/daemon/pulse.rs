use std::time::Duration;

use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use tokio::time::Instant;
use ts_rs::TS;

const BEAT: Duration = Duration::from_secs(15);
const FRONT_BEAT: Duration = Duration::from_secs(5);
const BACK_BEAT: Duration = Duration::from_secs(30);
const UNANSWERED_LIMIT: u32 = 2;
const QUIET_BEATS: f32 = 2.5;

/// 화면이 웹소켓에 보내는 글 프레임. 데몬은 이 값에 맞춰 박동 간격을 바꾸고 바로 박동을 보낸다.
/// 보내지 않은 화면에는 15초마다 박동을 보낸다.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, TS)]
#[serde(tag = "pace", rename_all = "camelCase")]
#[ts(export)]
pub enum Pace {
    /// 사람이 화면을 보고 있다.
    Front,
    /// 화면이 뒤에 있다. 휴대폰 서비스는 늘 이것을 보낸다.
    Back,
}

pub(super) struct Pulse {
    every: Duration,
    sent: Instant,
    pinged: Instant,
    heard: Instant,
}

impl Pulse {
    pub(super) fn new() -> Self {
        let now = Instant::now();
        Self {
            every: BEAT,
            sent: now,
            pinged: now,
            heard: now,
        }
    }

    pub(super) fn paced(&mut self, text: &str) -> bool {
        let Ok(pace) = serde_json::from_str(text) else {
            return false;
        };
        self.every = match pace {
            Pace::Front => FRONT_BEAT,
            Pace::Back => BACK_BEAT,
        };
        true
    }

    pub(super) fn quiet_ms(&self) -> u32 {
        u32::try_from(self.every.mul_f32(QUIET_BEATS).as_millis()).unwrap_or(u32::MAX)
    }

    pub(super) fn sent(&mut self) {
        self.sent = Instant::now();
    }

    pub(super) async fn listen<S: Stream + Unpin>(&mut self, from: &mut S) -> Heard<S::Item> {
        let (at, beat) = self.next();
        tokio::select! {
            biased;
            frame = from.next() => {
                if frame.is_some() {
                    self.heard = Instant::now();
                }
                Heard::Frame(frame)
            }
            () = tokio::time::sleep_until(at) => {
                let now = Instant::now();
                match beat {
                    Beat::Send => (self.sent, self.pinged) = (now, now),
                    Beat::Ping => self.pinged = now,
                    Beat::Dead => {}
                }
                Heard::Beat(beat)
            }
        }
    }

    fn next(&self) -> (Instant, Beat) {
        let dead = self.heard + self.every * (UNANSWERED_LIMIT + 1);
        let idle = self.sent + self.every;
        let ping = self.pinged.max(self.heard) + self.every;
        if dead <= idle.min(ping) {
            (dead, Beat::Dead)
        } else if idle <= ping {
            (idle, Beat::Send)
        } else {
            (ping, Beat::Ping)
        }
    }
}

pub(super) enum Heard<T> {
    Frame(Option<T>),
    Beat(Beat),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Beat {
    Send,
    Ping,
    Dead,
}
