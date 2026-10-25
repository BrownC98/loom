use std::time::Duration;

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

    pub(super) fn heard(&mut self) {
        self.unanswered = 0;
    }

    pub(super) async fn due(&mut self) -> Beat {
        self.tick.tick().await;
        self.unanswered += 1;
        if self.unanswered > UNANSWERED_LIMIT {
            Beat::Dead
        } else {
            Beat::Send
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Beat {
    Send,
    Dead,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn a_peer_that_never_answers_is_dead_after_the_limit() {
        let mut pulse = Pulse::new(BEAT);
        let start = Instant::now();
        assert_eq!(pulse.due().await, Beat::Send);
        assert_eq!(pulse.due().await, Beat::Send);
        assert_eq!(pulse.due().await, Beat::Dead);
        assert_eq!(start.elapsed(), BEAT * 3);
    }

    #[tokio::test(start_paused = true)]
    async fn an_answer_keeps_the_peer_alive() {
        let mut pulse = Pulse::new(BEAT);
        for _ in 0..10 {
            assert_eq!(pulse.due().await, Beat::Send);
            pulse.heard();
        }
    }
}
