use std::time::Duration;

use futures_util::{Stream, StreamExt};
use tokio::time::{Instant, Interval, MissedTickBehavior};

pub(super) const BEAT: Duration = Duration::from_secs(15);
const UNANSWERED_LIMIT: u32 = 2;

pub(super) struct Pulse {
    tick: Interval,
    unanswered: u32,
}

impl Pulse {
    pub(super) fn new(every: Duration) -> Self {
        let mut tick = tokio::time::interval_at(Instant::now() + every, every);
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        Self {
            tick,
            unanswered: 0,
        }
    }

    pub(super) async fn listen<S: Stream + Unpin>(&mut self, from: &mut S) -> Heard<S::Item> {
        tokio::select! {
            biased;
            frame = from.next() => {
                if frame.is_some() {
                    self.unanswered = 0;
                }
                Heard::Frame(frame)
            }
            beat = self.due() => Heard::Beat(beat),
        }
    }

    async fn due(&mut self) -> Beat {
        self.tick.tick().await;
        self.unanswered += 1;
        if self.unanswered > UNANSWERED_LIMIT {
            Beat::Dead
        } else {
            Beat::Send
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
}
