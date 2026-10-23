//! 계정 전환: 실행 중인 노드를 새 계정으로 다시 띄워 같은 세션을 잇고, 켜져 있으면 한도에 닿은 계정을 자동으로 바꾼다.
//! 근거: docs/design.md 「계정 전환」, 목업 docs/mockups/account-switch.html.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Path as UrlPath, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::relaunch::{Relaunched, provider_of};
use super::usage_cache::{Key, UsageCache};
use super::{App, Problem, internal};
use crate::accounts::{AccountId, Site};
use crate::graph::{Active, Entry, NodeId, SwitchCause};
use crate::profile::Provider;
use crate::usage::{self, Limits, Window};
use crate::wsl;

/// 이 비율(%)을 쓴 창이 있는 계정은 다 썼다. 그 계정으로는 바꾸지 않는다.
const EXHAUSTED_AT: u8 = 100;
/// 같은 공급자를 다시 자동 전환하지 않는 시간. 같은 계정의 노드 여럿이 한꺼번에 한도로 멈추면
/// 멈춤마다 다시 바꾸어 계정을 오가는 일을 막는다. 「남은 계정 없음」 기록도 이 간격에 한 번만 남긴다.
const COOLDOWN: Duration = Duration::from_secs(10 * 60);

/// 계정을 바꿀 때 옮길 수 있는 실행 중인 노드(`GET /api/accounts/{provider}/nodes`). 확인 창 목록이다(목업 ①).
/// 지금 계정이 설정의 계정과 같은 노드는 넣지 않는다.
#[derive(Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MovableNode {
    pub id: NodeId,
    pub name: String,
    /// 턴 중이다. 그 턴이 끝난 뒤 옮긴다.
    pub busy: bool,
}

/// `POST /api/accounts/{provider}/move`의 결과. 설정에 저장한 계정으로 옮긴다. 같은 값을 이벤트 기록에도 남긴다.
#[derive(Clone, Copy, Default, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MoveResult {
    pub moved: u32,
    pub later: u32,
    pub failed: u32,
}

/// 자동 전환 기록. 기억에만 둔다.
#[derive(Default)]
pub struct Switches {
    /// 위치(배포판, 없으면 이 PC)와 공급자마다 마지막 자동 전환이나 「남은 계정 없음」 기록 시각.
    last_auto: Mutex<HashMap<(Provider, Option<String>), Instant>>,
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/accounts/{provider}/nodes", get(movable))
        .route("/api/accounts/{provider}/move", post(move_all))
}

/// `GET /api/accounts/{provider}/nodes`의 질의. `to`는 옮겨 갈 계정이다. 없으면 시스템 기본이다.
/// `distro`는 계정의 위치다. 없으면 이 PC다. 그 위치 저장소의 노드만 옮긴다.
/// 화면은 설정을 저장하기 전에 확인 창 목록을 읽는다.
#[derive(Deserialize)]
struct MovableQuery {
    to: Option<AccountId>,
    distro: Option<String>,
}

/// `POST /api/accounts/{provider}/move`의 질의. 그 위치의 노드를 그 위치의 「사용 중」 계정으로 옮긴다.
#[derive(Deserialize)]
struct MoveQuery {
    distro: Option<String>,
}

async fn movable(
    State(app): State<Arc<App>>,
    UrlPath(provider): UrlPath<Provider>,
    Query(query): Query<MovableQuery>,
) -> Result<Json<Vec<MovableNode>>, Problem> {
    let nodes = app
        .movable(provider, query.distro.as_deref(), query.to.as_ref())
        .map_err(internal)?;
    Ok(Json(
        nodes
            .into_iter()
            .map(|n| MovableNode {
                busy: app.busy(n.id),
                id: n.id,
                name: n.name,
            })
            .collect(),
    ))
}

/// 화면이 설정을 먼저 저장한 뒤 부른다.
async fn move_all(
    State(app): State<Arc<App>>,
    UrlPath(provider): UrlPath<Provider>,
    Query(query): Query<MoveQuery>,
) -> Result<Json<MoveResult>, Problem> {
    let distro = query.distro.as_deref();
    Ok(Json(
        app.switch_nodes(provider, distro, SwitchCause::Manual)
            .await?,
    ))
}

impl App {
    /// 위치 `distro`(없으면 이 PC)에서 실행 중인 그 공급자 노드 가운데 `target` 계정과 다른 계정으로 띄운 노드. `None`은 시스템 기본이다.
    fn movable(
        &self,
        provider: Provider,
        distro: Option<&str>,
        target: Option<&AccountId>,
    ) -> rusqlite::Result<Vec<Active>> {
        Ok(self
            .graph
            .active()?
            .into_iter()
            .filter(|n| provider_of(n) == Some(provider))
            .filter(|n| n.account.as_ref() != target && !self.relaunches.exiting(n.id))
            .filter(|n| self.distro_of(n.id).as_deref() == distro)
            .collect())
    }

    /// 노드가 도는 WSL 배포판. 이 PC 노드거나 자리를 읽지 못하면 없다.
    fn distro_of(&self, node: NodeId) -> Option<String> {
        let place = self.graph.place_of(node).ok()?;
        wsl::of(Path::new(&place.worktree)).map(|at| at.distro)
    }

    /// 옮길 노드를 모두 설정의 계정으로 옮기고 이벤트 기록에 한 줄을 남긴다.
    /// `from`은 옮길 노드가 가장 많이 쓰던 계정이다. 같은 수면 번호가 작은 노드의 계정이다.
    async fn switch_nodes(
        self: &Arc<Self>,
        provider: Provider,
        distro: Option<&str>,
        why: SwitchCause,
    ) -> Result<MoveResult, Problem> {
        let to = self
            .settings
            .get()
            .map_err(internal)?
            .account(provider, distro)
            .cloned();
        let nodes = self
            .movable(provider, distro, to.as_ref())
            .map_err(internal)?;
        let from = most_used(nodes.iter().map(|n| n.account.clone()));
        let outcomes = join_all(nodes.into_iter().map(|node| self.relaunch_or_queue(node))).await;
        let mut result = MoveResult::default();
        for outcome in outcomes {
            match outcome {
                Ok(Relaunched::Now) => result.moved += 1,
                Ok(Relaunched::AtTurnEnd) => result.later += 1,
                Err(e) => {
                    eprintln!("loom: 노드를 새 계정으로 옮기지 못했다: {e}");
                    result.failed += 1;
                }
            }
        }
        self.graph
            .record(&Entry::AccountSwitched {
                provider,
                distro: distro.map(str::to_owned),
                from,
                to,
                why,
                moved: result.moved,
                later: result.later,
                failed: result.failed,
            })
            .map_err(internal)?;
        Ok(result)
    }

    /// 노드가 한도로 턴을 멈췄다. 옮길 때 이어 가라는 입력을 넣는다. 자동 전환이 켜져 있으면 바로 바꾼다.
    /// 자동 전환은 이 이벤트로만 시작한다. 사용량을 미리 읽어 바꾸지 않는다.
    pub(super) fn rate_limited(self: &Arc<Self>, node: NodeId, provider: Provider) {
        self.relaunches.stopped_at_limit(node);
        let app = self.clone();
        let distro = self.distro_of(node);
        tokio::spawn(async move { app.auto_switch(provider, distro).await });
    }

    /// 자동 전환이 켜져 있으면 같은 위치(`distro`, 없으면 이 PC), 같은 공급자의 다른 계정 사용량을 그 자리에서 읽고, 남은 계정으로 바꾼다.
    async fn auto_switch(self: &Arc<Self>, provider: Provider, distro: Option<String>) {
        let Ok(settings) = self.settings.get() else {
            return;
        };
        let key = (provider, distro.clone());
        if !settings.auto_switch || self.cooling_down(&key) {
            return;
        }
        let site = match self.site_of(distro.as_deref()).await {
            Ok(site) => Arc::new(site),
            Err(e) => {
                eprintln!("loom: 자동 전환할 위치를 읽지 못했다: {e:?}");
                return;
            }
        };
        let current = settings.account(provider, distro.as_deref()).cloned();
        let mut ids = vec![None];
        ids.extend(
            site.accounts()
                .list(provider)
                .unwrap_or_default()
                .into_iter()
                .map(Some),
        );
        ids.retain(|id| *id != current);
        let probes = join_all(
            ids.iter()
                .map(|id| probe(&self.usage, &site, provider, id.clone())),
        )
        .await;
        let candidates: Vec<_> = ids.into_iter().zip(probes).collect();
        self.switch
            .last_auto
            .lock()
            .unwrap()
            .insert(key, Instant::now());
        let Some(to) = pick_target(&candidates) else {
            let resets_at = earliest_reset(candidates.iter().map(|(_, l)| l));
            if let Err(e) = self.graph.record(&Entry::NoAccountLeft {
                provider,
                distro: distro.clone(),
                resets_at,
            }) {
                eprintln!("loom: 「남은 계정 없음」을 남기지 못했다: {e}");
            }
            return;
        };
        // 사용량을 읽는 동안 화면이 다른 설정을 바꿨을 수 있다. 계정 칸만 바꾼다.
        if let Err(e) = self
            .settings
            .update(|view| view.set_account(provider, distro.as_deref(), to))
        {
            eprintln!("loom: 자동 전환한 계정을 저장하지 못했다: {e}");
            return;
        }
        let switched = self
            .switch_nodes(provider, distro.as_deref(), SwitchCause::RateLimit)
            .await;
        if let Err(e) = switched {
            eprintln!("loom: 자동 전환으로 노드를 옮기지 못했다: {e:?}");
        }
    }

    fn cooling_down(&self, key: &(Provider, Option<String>)) -> bool {
        self.switch
            .last_auto
            .lock()
            .unwrap()
            .get(key)
            .is_some_and(|at| at.elapsed() < COOLDOWN)
    }
}

async fn probe(
    cache: &UsageCache,
    site: &Arc<Site>,
    provider: Provider,
    id: Option<AccountId>,
) -> Limits {
    let key = Key::provider(site, provider, id.as_ref());
    let site = site.clone();
    let fingerprint = usage::credentials_fingerprint(provider, &site, id.as_ref());
    cache
        .get(key, fingerprint, move || {
            usage::probe(provider, &site, id.as_ref())
        })
        .await
        .limits
}

/// 가장 많이 나온 계정. 같은 수면 먼저 나온 계정이다. 비었거나 시스템 기본이 가장 많으면 없다.
fn most_used(accounts: impl Iterator<Item = Option<AccountId>>) -> Option<AccountId> {
    let accounts: Vec<_> = accounts.collect();
    let count = |a: &Option<AccountId>| accounts.iter().filter(|b| *b == a).count();
    let mut best: Option<&Option<AccountId>> = None;
    for a in &accounts {
        if best.is_none_or(|b| count(a) > count(b)) {
            best = Some(a);
        }
    }
    best.cloned().flatten()
}

/// 사용량을 읽은 계정 가운데 가장 많이 쓴 창이 가장 적은 계정. 그 값이 `EXHAUSTED_AT` 미만이어야 한다.
/// 읽지 못한 계정은 건너뛴다. 같은 값이면 앞 계정(시스템 기본, 만든 순서)이다.
fn pick_target(candidates: &[(Option<AccountId>, Limits)]) -> Option<Option<AccountId>> {
    candidates
        .iter()
        .filter_map(|(id, limits)| match limits {
            Limits::Ready {
                session, weekly, ..
            } => {
                let used = [session, weekly]
                    .into_iter()
                    .flatten()
                    .map(|w| w.used)
                    .max()
                    .unwrap_or(0);
                (used < EXHAUSTED_AT).then_some((id, used))
            }
            _ => None,
        })
        .min_by_key(|(_, used)| *used)
        .map(|(id, _)| id.clone())
}

/// 계정들의 창 가운데 가장 빠른 초기화 시각.
fn earliest_reset<'a>(limits: impl Iterator<Item = &'a Limits>) -> Option<i64> {
    limits
        .filter_map(|l| match l {
            Limits::Ready {
                session, weekly, ..
            } => Some([session, weekly]),
            _ => None,
        })
        .flatten()
        .flatten()
        .filter_map(|w: &Window| w.resets_at)
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(session: u8, weekly: u8) -> Limits {
        Limits::Ready {
            session: Some(Window {
                used: session,
                resets_at: Some(100 + i64::from(session)),
            }),
            weekly: Some(Window {
                used: weekly,
                resets_at: Some(1000),
            }),
            models: Vec::new(),
            plan: None,
        }
    }

    fn id(n: u8) -> Option<AccountId> {
        serde_json::from_value(serde_json::json!(format!("{n:016x}"))).unwrap()
    }

    #[test]
    fn target_is_the_least_used_readable_account_with_quota_left() {
        let candidates = vec![
            (None, ready(50, 60)),
            (id(1), Limits::NeedsLogin),
            (id(2), ready(10, 40)),
            (id(3), ready(10, 40)),
            (id(4), ready(0, 100)),
        ];
        assert_eq!(pick_target(&candidates), Some(id(2)));
        let nearly = vec![(None, ready(100, 0)), (id(1), ready(99, 99))];
        assert_eq!(pick_target(&nearly), Some(id(1)));
        let full = vec![(None, ready(100, 0)), (id(1), Limits::NeedsLogin)];
        assert_eq!(pick_target(&full), None);
        assert_eq!(earliest_reset(full.iter().map(|(_, l)| l)), Some(200));
    }

    #[test]
    fn from_is_the_account_most_nodes_had() {
        assert_eq!(most_used([id(1), None, None].into_iter()), None);
        assert_eq!(most_used([id(1), id(2), id(2)].into_iter()), id(2));
        assert_eq!(most_used([id(2), id(1)].into_iter()), id(2));
        assert_eq!(most_used(std::iter::empty()), None);
    }
}
