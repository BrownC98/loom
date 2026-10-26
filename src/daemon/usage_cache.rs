use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::accounts::{AccountId, Site};
use crate::protocol::Provider;
use crate::usage::{Limits, Usage, UsageProblem};

const MIN_AGE: Duration = Duration::from_secs(60);

#[derive(Clone, PartialEq, Eq, Hash)]
enum Source {
    Provider(Provider),
    Cursor,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Key {
    distro: Option<String>,
    source: Source,
    id: Option<AccountId>,
}

impl Key {
    pub fn provider(site: &Site, provider: Provider, id: Option<&AccountId>) -> Self {
        Key {
            distro: site.distro().map(str::to_owned),
            source: Source::Provider(provider),
            id: id.cloned(),
        }
    }

    pub fn cursor(site: &Site) -> Self {
        Key {
            distro: site.distro().map(str::to_owned),
            source: Source::Cursor,
            id: None,
        }
    }
}

#[derive(Default)]
struct Slot {
    fingerprint: Option<u64>,
    good: Option<(Usage, Instant)>,
    failed: Option<(Usage, Instant)>,
    blocked: Option<(Instant, i64)>,
}

impl Slot {
    fn good_or(&self, failure: Usage) -> Usage {
        match &self.good {
            Some((usage, _)) => usage.clone(),
            None => failure,
        }
    }
}

fn transient(problem: &UsageProblem) -> bool {
    matches!(
        problem,
        UsageProblem::Unavailable(_)
            | UsageProblem::ResponseFormat
            | UsageProblem::NoReply(_)
            | UsageProblem::CliFailed { .. }
    )
}

pub struct UsageCache {
    min_age: Duration,
    slots: Mutex<HashMap<Key, Arc<tokio::sync::Mutex<Slot>>>>,
}

impl Default for UsageCache {
    fn default() -> Self {
        Self::new(MIN_AGE)
    }
}

impl UsageCache {
    fn new(min_age: Duration) -> Self {
        Self {
            min_age,
            slots: Mutex::default(),
        }
    }

    pub async fn get(
        &self,
        key: Key,
        fingerprint: Option<u64>,
        probe: impl FnOnce() -> Usage + Send + 'static,
    ) -> Usage {
        let slot = self.slots.lock().unwrap().entry(key).or_default().clone();
        let mut slot = slot.lock().await;
        if slot.fingerprint != fingerprint {
            *slot = Slot {
                fingerprint,
                ..Slot::default()
            };
        }
        if let Some((usage, at)) = &slot.good
            && at.elapsed() < self.min_age
        {
            return usage.clone();
        }
        if let Some((usage, at)) = &slot.failed
            && at.elapsed() < self.min_age
        {
            return slot.good_or(usage.clone());
        }
        if let Some((until, retry_at)) = slot.blocked {
            if Instant::now() < until {
                return slot.good_or(Usage::failed(UsageProblem::RateLimited { retry_at }));
            }
            slot.blocked = None;
        }
        let usage = tokio::task::spawn_blocking(probe)
            .await
            .unwrap_or_else(|e| Usage::failed(UsageProblem::Unavailable(e.to_string())));
        slot.failed = None;
        match &usage.limits {
            Limits::Ready { .. } => {
                slot.good = Some((usage.clone(), Instant::now()));
                usage
            }
            Limits::Failed {
                problem: UsageProblem::RateLimited { retry_at },
            } => {
                let wait = (retry_at - jiff::Timestamp::now().as_second()).max(0) as u64;
                slot.blocked = Some((Instant::now() + Duration::from_secs(wait), *retry_at));
                slot.good_or(usage)
            }
            Limits::Failed { problem } if transient(problem) => {
                slot.failed = Some((usage.clone(), Instant::now()));
                slot.good_or(usage)
            }
            _ => {
                slot.good = None;
                usage
            }
        }
    }
}
