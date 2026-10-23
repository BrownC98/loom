//! 사용량 캐시: 공급자 API를 읽는 곳은 여기 하나다. 화면 수와 새로 고침 수에 상관없이
//! 같은 계정을 `MIN_AGE`보다 자주 읽지 않는다. 바뀌는 이유는 공급자의 호출 한도다.
//! 근거: docs/design.md 「계정과 사용량」·「실측 근거」.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::accounts::{AccountId, Site};
use crate::profile::Provider;
use crate::usage::{Limits, Usage, UsageProblem};

/// 같은 계정을 다시 읽기 전에 기다리는 최소 시간. 2026-10-23 실측에서 `api/oauth/usage`는
/// 토큰 하나에 2초 안의 6번째 호출부터 429(`retry-after: 300`)를 줬다. 1분이면 분당 1번이라
/// 한도와 멀고, 사람이 새로 고침을 눌러도 1분 전 값은 본다. Orca·Claude Code가 같은 토큰으로 쓰는 몫도 남는다.
const MIN_AGE: Duration = Duration::from_secs(60);

#[derive(Clone, PartialEq, Eq, Hash)]
enum Source {
    Provider(Provider),
    Cursor,
}

/// 읽는 곳: 위치(이 PC 또는 WSL 배포판), 출처, 계정(시스템 기본은 없음).
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
    /// 마지막으로 읽은 사용 가능한 값과 읽은 때.
    good: Option<(Usage, Instant)>,
    /// 429를 받은 뒤 다시 읽을 수 있는 때와 그 유닉스 초.
    blocked: Option<(Instant, i64)>,
}

pub struct UsageCache {
    min_age: Duration,
    /// 키마다 비동기 잠금 하나. 잠금을 잡은 채 읽으므로 같은 키의 동시 요청은 한 번만 읽는다.
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

    /// 계정 `key`의 사용량. `probe`는 캐시가 새 값이 필요할 때만 막는 스레드에서 돈다.
    pub async fn get(&self, key: Key, probe: impl FnOnce() -> Usage + Send + 'static) -> Usage {
        let slot = self.slots.lock().unwrap().entry(key).or_default().clone();
        let mut slot = slot.lock().await;
        if let Some((usage, at)) = &slot.good
            && at.elapsed() < self.min_age
        {
            return usage.clone();
        }
        if let Some((until, retry_at)) = slot.blocked {
            if Instant::now() < until {
                return match &slot.good {
                    Some((usage, _)) => usage.clone(),
                    None => Usage::failed(UsageProblem::RateLimited { retry_at }),
                };
            }
            slot.blocked = None;
        }
        let usage = tokio::task::spawn_blocking(probe)
            .await
            .unwrap_or_else(|e| Usage::failed(UsageProblem::Unavailable(e.to_string())));
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
                match &slot.good {
                    Some((good, _)) => good.clone(),
                    None => usage,
                }
            }
            // 로그아웃·만료·미설치는 앞 값이 이 계정의 것이 아니다. 지워서 다음 429에 보이지 않게 한다.
            _ => {
                slot.good = None;
                usage
            }
        }
    }

    /// 로그아웃하거나 지운 계정의 앞 값을 버린다. 다음 요청이 바로 다시 읽는다.
    pub fn forget(&self, key: &Key) {
        self.slots.lock().unwrap().remove(key);
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

    /// 화면 20개가 동시에 부르고 다시 새로 고쳐도 공급자는 한 번만 불린다.
    #[tokio::test]
    async fn many_callers_make_one_probe_per_min_age() {
        let cache = UsageCache::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let read = || {
            let calls = calls.clone();
            cache.get(key(), move || {
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
                .get(key, move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    ready(10)
                })
                .await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    /// 429 뒤에는 `retry-after`가 끝날 때까지 부르지 않고, 앞 값을 읽은 때와 함께 준다.
    #[tokio::test]
    async fn rate_limit_keeps_last_good_and_stops_calls() {
        let cache = UsageCache::new(Duration::ZERO);
        let good = cache.get(key(), || ready(42)).await;
        let calls = Arc::new(AtomicUsize::new(0));
        for _ in 0..4 {
            let calls = calls.clone();
            let shown = cache
                .get(key(), move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    limited(300)
                })
                .await;
            assert_eq!(used(&shown), Some(42));
            assert_eq!(shown.read_at, good.read_at);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// 앞 값이 없으면 한도 문제(`RateLimited`)를 준다. 막힌 동안은 다시 부르지 않는다.
    #[tokio::test]
    async fn rate_limit_without_good_value_is_typed() {
        let cache = UsageCache::new(Duration::ZERO);
        let calls = Arc::new(AtomicUsize::new(0));
        for _ in 0..3 {
            let calls = calls.clone();
            let shown = cache
                .get(key(), move || {
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

    /// `retry-after`가 끝나면 다음 요청이 다시 읽는다.
    #[tokio::test]
    async fn reads_again_after_retry_time() {
        let cache = UsageCache::new(Duration::ZERO);
        cache.get(key(), || ready(42)).await;
        cache.get(key(), || limited(0)).await;
        let shown = cache.get(key(), || ready(50)).await;
        assert_eq!(used(&shown), Some(50));
    }

    /// 로그아웃(미로그인)한 계정의 앞 값은 뒤의 429에 다시 나오지 않는다.
    #[tokio::test]
    async fn logged_out_drops_last_good() {
        let cache = UsageCache::new(Duration::ZERO);
        cache.get(key(), || ready(42)).await;
        cache
            .get(key(), || Usage::new(None, None, Limits::NeedsLogin))
            .await;
        let shown = cache.get(key(), || limited(300)).await;
        assert_eq!(used(&shown), None);
    }

    #[tokio::test]
    async fn forget_reads_at_once() {
        let cache = UsageCache::default();
        cache.get(key(), || ready(1)).await;
        cache.forget(&key());
        let shown = cache.get(key(), || ready(2)).await;
        assert_eq!(used(&shown), Some(2));
    }
}
