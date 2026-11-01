use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::time::{Duration, Instant};

use api::graph::{GraphEvent, Toward};

use super::hops::{HopEndName, pick_hue};

pub const LIVE: Duration = Duration::from_secs(3);
pub const FLOW: Duration = Duration::from_millis(600);

pub fn slot(toward: Toward) -> usize {
    match toward {
        Toward::Parent => 0,
        Toward::Child => 1,
    }
}

pub struct Pulse {
    pub hue: f64,
    pub last: Toward,
    pub counts: [u32; 2],
    until: Instant,
}

pub struct Hop {
    pub source: u32,
    pub target: u32,
    pub hue: f64,
    pub last: HopEndName,
    pub counts: [u32; 2],
    until: Instant,
}

#[derive(Default)]
pub struct Traffic {
    pub pulses: HashMap<u32, Pulse>,
    pub hops: HashMap<String, Hop>,
}

fn random() -> f64 {
    RandomState::new().hash_one(0u8) as f64 / u64::MAX as f64
}

impl Traffic {
    pub fn is_empty(&self) -> bool {
        self.pulses.is_empty() && self.hops.is_empty()
    }

    fn shown(&self) -> Vec<f64> {
        self.pulses
            .values()
            .map(|p| p.hue)
            .chain(self.hops.values().map(|h| h.hue))
            .collect()
    }

    pub fn event(&mut self, event: &GraphEvent, now: Instant) {
        match event {
            GraphEvent::Message { child, toward } => {
                let prev = self.pulses.get(&child.0);
                let hue = prev.map_or_else(|| pick_hue(&self.shown(), random), |p| p.hue);
                let mut counts = prev.map_or([0, 0], |p| p.counts);
                counts[slot(*toward)] += 1;
                self.pulses.insert(
                    child.0,
                    Pulse {
                        hue,
                        last: *toward,
                        counts,
                        until: now + LIVE,
                    },
                );
            }
            GraphEvent::Hop { from, to } => {
                let (source, target) = (from.0.min(to.0), from.0.max(to.0));
                let key = format!("{source}-{target}");
                let toward = if to.0 == target {
                    HopEndName::Target
                } else {
                    HopEndName::Source
                };
                let prev = self.hops.get(&key);
                let hue = prev.map_or_else(|| pick_hue(&self.shown(), random), |h| h.hue);
                let mut counts = prev.map_or([0, 0], |h| h.counts);
                counts[toward as usize] += 1;
                self.hops.insert(
                    key,
                    Hop {
                        source,
                        target,
                        hue,
                        last: toward,
                        counts,
                        until: now + LIVE,
                    },
                );
            }
            _ => {}
        }
    }

    pub fn prune(&mut self, now: Instant) {
        self.pulses.retain(|_, p| p.until > now);
        self.hops.retain(|_, h| h.until > now);
    }
}
