use std::time::Duration;

use futures_util::{Stream, StreamExt};
use tokio::time::Instant;

pub(super) const BEAT: Duration = Duration::from_secs(15);
pub(super) const FRONT_BEAT: Duration = Duration::from_secs(5);
pub(super) const BACK_BEAT: Duration = Duration::from_secs(30);
const UNANSWERED_LIMIT: u32 = 2;

pub(super) struct Pulse {
    every: Duration,
    sent: Instant,
    pinged: Instant,
    heard: Instant,
}

impl Pulse {
    pub(super) fn new(every: Duration) -> Self {
        let now = Instant::now();
        Self {
            every,
            sent: now,
            pinged: now,
            heard: now,
        }
    }

    pub(super) fn pace(&mut self, every: Duration) {
        self.every = every;
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
