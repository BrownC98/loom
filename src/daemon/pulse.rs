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

#[cfg(test)]
mod tests {
    use super::*;

    fn beat(heard: Heard<()>) -> Option<Beat> {
        match heard {
            Heard::Beat(beat) => Some(beat),
            Heard::Frame(_) => None,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_peer_that_never_answers_is_dead_after_the_limit() {
        let mut pulse = Pulse::new(BEAT);
        let mut silent = futures_util::stream::pending::<()>();
        let start = Instant::now();
        assert_eq!(beat(pulse.listen(&mut silent).await), Some(Beat::Send));
        assert_eq!(beat(pulse.listen(&mut silent).await), Some(Beat::Send));
        assert_eq!(beat(pulse.listen(&mut silent).await), Some(Beat::Dead));
        assert_eq!(start.elapsed(), BEAT * 3);
    }

    #[tokio::test(start_paused = true)]
    async fn an_answer_queued_behind_a_slow_send_keeps_the_peer_alive() {
        let mut pulse = Pulse::new(BEAT);
        let (answers, mut queued) = tokio::sync::mpsc::unbounded_channel();
        let mut from = futures_util::stream::poll_fn(move |cx| queued.poll_recv(cx));
        for _ in 0..10 {
            assert_eq!(beat(pulse.listen(&mut from).await), Some(Beat::Send));
            answers.send(()).unwrap();
            tokio::time::advance(BEAT).await;
            assert!(matches!(
                pulse.listen(&mut from).await,
                Heard::Frame(Some(()))
            ));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_pace_sets_the_beat_interval() {
        let mut silent = futures_util::stream::pending::<()>();
        for every in [FRONT_BEAT, BEAT, BACK_BEAT] {
            let mut pulse = Pulse::new(BEAT);
            pulse.pace(every);
            let start = Instant::now();
            assert_eq!(beat(pulse.listen(&mut silent).await), Some(Beat::Send));
            assert_eq!(start.elapsed(), every);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_sent_frame_puts_off_the_next_beat() {
        let mut pulse = Pulse::new(FRONT_BEAT);
        let (answers, mut queued) = tokio::sync::mpsc::unbounded_channel();
        let mut from = futures_util::stream::poll_fn(move |cx| queued.poll_recv(cx));
        let start = Instant::now();
        tokio::time::advance(FRONT_BEAT - Duration::from_secs(1)).await;
        pulse.sent();
        answers.send(()).unwrap();
        assert!(matches!(
            pulse.listen(&mut from).await,
            Heard::Frame(Some(()))
        ));
        assert_eq!(beat(pulse.listen(&mut from).await), Some(Beat::Send));
        assert_eq!(start.elapsed(), FRONT_BEAT * 2 - Duration::from_secs(1));
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_peer_is_dead_after_the_limit_while_frames_flow() {
        let mut pulse = Pulse::new(FRONT_BEAT);
        let mut silent = futures_util::stream::pending::<()>();
        let start = Instant::now();
        let mut beats = Vec::new();
        loop {
            let flowing =
                tokio::time::timeout(Duration::from_secs(1), pulse.listen(&mut silent)).await;
            match flowing {
                Err(_) => pulse.sent(),
                Ok(heard) => {
                    let beat = beat(heard).unwrap();
                    if beat == Beat::Dead {
                        break;
                    }
                    beats.push(beat);
                }
            }
        }
        assert_eq!(beats, [Beat::Ping, Beat::Ping]);
        assert_eq!(start.elapsed(), FRONT_BEAT * (UNANSWERED_LIMIT + 1));
    }

    #[tokio::test(start_paused = true)]
    async fn a_peer_that_answers_pings_stays_alive_while_frames_flow() {
        let mut pulse = Pulse::new(FRONT_BEAT);
        let (answers, mut queued) = tokio::sync::mpsc::unbounded_channel();
        let mut from = futures_util::stream::poll_fn(move |cx| queued.poll_recv(cx));
        let until = Instant::now() + FRONT_BEAT * 10;
        let mut pings = 0;
        while Instant::now() < until {
            match tokio::time::timeout(Duration::from_secs(1), pulse.listen(&mut from)).await {
                Err(_) => pulse.sent(),
                Ok(Heard::Beat(Beat::Ping)) => {
                    pings += 1;
                    answers.send(()).unwrap();
                }
                Ok(Heard::Frame(_)) => {}
                Ok(Heard::Beat(beat)) => panic!("{beat:?}"),
            }
        }
        assert!(pings >= 8, "{pings} pings");
    }
}
