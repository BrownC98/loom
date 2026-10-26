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
use crate::protocol::Provider;
use crate::usage::{self, Limits, Window};
use crate::wsl;

const EXHAUSTED_AT: u8 = 100;
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

#[derive(Default)]
pub struct Switches {
    last_auto: Mutex<HashMap<(Provider, Option<String>), Instant>>,
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/accounts/{provider}/nodes", get(movable))
        .route("/api/accounts/{provider}/move", post(move_all))
}

#[derive(Deserialize)]
struct MovableQuery {
    to: Option<AccountId>,
    distro: Option<String>,
}

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

    fn distro_of(&self, node: NodeId) -> Option<String> {
        let place = self.graph.place_of(node).ok()?;
        wsl::of(Path::new(&place.worktree)).map(|at| at.distro)
    }

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

    pub(super) fn rate_limited(self: &Arc<Self>, node: NodeId, provider: Provider) {
        if let Err(e) = self.graph.limit_reached(node) {
            eprintln!("loom: 노드 {}의 한도 표시를 쓰지 못했다: {e}", node.0);
        }
        let app = self.clone();
        let distro = self.distro_of(node);
        tokio::spawn(async move { app.auto_switch(provider, distro).await });
    }

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
