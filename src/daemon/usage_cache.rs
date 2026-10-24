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

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::future::join_all;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn key() -> Key {
        Key {
            distro: None,
            source: Source::Provider(Provider::Claude),
            id: None,
        }
    }

    fn ready(used: u8) -> Usage {
        Usage::new(
            Some("a@b.c".into()),
            None,
            Limits::Ready {
                session: Some(crate::usage::Window {
                    used,
                    resets_at: None,
                }),
                weekly: None,
                models: Vec::new(),
                plan: None,
            },
        )
    }

    fn limited(secs: i64) -> Usage {
        Usage::failed(UsageProblem::RateLimited {
            retry_at: jiff::Timestamp::now().as_second() + secs,
        })
    }

    fn used(usage: &Usage) -> Option<u8> {
        match &usage.limits {
            Limits::Ready {
                session: Some(w), ..
            } => Some(w.used),
            _ => None,
        }
    }

    #[tokio::test]
    async fn many_callers_make_one_probe_per_min_age() {
        let cache = UsageCache::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let read = || {
            let calls = calls.clone();
            cache.get(key(), None, move || {
                calls.fetch_add(1, Ordering::SeqCst);
                ready(10)
            })
        };
        join_all((0..20).map(|_| read())).await;
        for _ in 0..5 {
            read().await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn other_keys_read_on_their_own() {
        let cache = UsageCache::default();
        let calls = Arc::new(AtomicUsize::new(0));
        for id in [
            None,
            Some(serde_json::from_str::<AccountId>("\"0123456789abcdef\"").unwrap()),
        ] {
            let calls = calls.clone();
            let key = Key { id, ..key() };
            cache
                .get(key, None, move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    ready(10)
                })
                .await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn rate_limit_keeps_last_good_and_stops_calls() {
        let cache = UsageCache::new(Duration::ZERO);
        let good = cache.get(key(), None, || ready(42)).await;
        let calls = Arc::new(AtomicUsize::new(0));
        for _ in 0..4 {
            let calls = calls.clone();
            let shown = cache
                .get(key(), None, move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    limited(300)
                })
                .await;
            assert_eq!(used(&shown), Some(42));
            assert_eq!(shown.read_at, good.read_at);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn rate_limit_without_good_value_is_typed() {
        let cache = UsageCache::new(Duration::ZERO);
        let calls = Arc::new(AtomicUsize::new(0));
        for _ in 0..3 {
            let calls = calls.clone();
            let shown = cache
                .get(key(), None, move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    limited(300)
                })
                .await;
            assert!(matches!(
                shown.limits,
                Limits::Failed {
                    problem: UsageProblem::RateLimited { .. }
                }
            ));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn reads_again_after_retry_time() {
        let cache = UsageCache::new(Duration::ZERO);
        cache.get(key(), None, || ready(42)).await;
        cache.get(key(), None, || limited(0)).await;
        let shown = cache.get(key(), None, || ready(50)).await;
        assert_eq!(used(&shown), Some(50));
    }

    #[tokio::test]
    async fn logged_out_drops_last_good() {
        let cache = UsageCache::new(Duration::ZERO);
        cache.get(key(), None, || ready(42)).await;
        cache
            .get(key(), None, || Usage::new(None, None, Limits::NeedsLogin))
            .await;
        let shown = cache.get(key(), None, || limited(300)).await;
        assert_eq!(used(&shown), None);
    }

    #[tokio::test]
    async fn changed_credentials_drop_last_good_and_backoff() {
        let cache = UsageCache::new(Duration::ZERO);
        let (a, b) = (Some(1), Some(2));
        cache.get(key(), a, || ready(42)).await;
        let shown = cache.get(key(), b, || limited(300)).await;
        assert_eq!(used(&shown), None);
        let shown = cache.get(key(), a, || ready(7)).await;
        assert_eq!(used(&shown), Some(7));
    }

    #[tokio::test]
    async fn transient_failure_is_not_retried_within_min_age() {
        let cache = UsageCache::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let read = || {
            let calls = calls.clone();
            cache.get(key(), None, move || {
                calls.fetch_add(1, Ordering::SeqCst);
                Usage::failed(UsageProblem::Unavailable("http status: 503".into()))
            })
        };
        join_all((0..20).map(|_| read())).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn transient_failure_keeps_last_good() {
        let cache = UsageCache::new(Duration::ZERO);
        let good = cache.get(key(), None, || ready(42)).await;
        let shown = cache
            .get(key(), None, || {
                Usage::failed(UsageProblem::Unavailable("x".into()))
            })
            .await;
        assert_eq!(used(&shown), Some(42));
        assert_eq!(shown.read_at, good.read_at);
    }
}
