//! 설정 창과 상태 줄의 라우트: 에이전트 탐지, 설정, 계정, 사용량, 로그인 창.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{ApiError, App, internal, serve_pty};
use crate::accounts::AccountId;
use crate::node_host::NodeHost;
use crate::profile::{self, AgentView, Profile};
use crate::settings::SettingsView;
use crate::usage::{self, Usage};

const PROVIDERS: [Profile; 2] = [Profile::Claude, Profile::Codex];

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LoginId(pub u32);

#[derive(Clone, Copy, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum LoginState {
    Running,
    Succeeded,
    Failed,
}

/// 계정 추가·다시 로그인 창의 PTY. 그래프에는 나타나지 않는다.
#[derive(Default)]
pub struct Logins {
    host: NodeHost<LoginId>,
    states: Mutex<HashMap<LoginId, LoginState>>,
    next: AtomicU32,
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AccountView {
    pub provider: Profile,
    /// 없으면 시스템 기본(`~/.claude`·`~/.codex`)이다. 쓰는 계정은 설정(`SettingsView`)이 정한다.
    pub id: Option<AccountId>,
    pub usage: Usage,
}

/// `id`가 있으면 그 계정에 다시 로그인한다. 없으면 계정을 새로 만든다.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct Login {
    pub id: Option<AccountId>,
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/agents", get(agents))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/accounts", get(accounts))
        .route("/api/accounts/{provider}", post(login))
        .route("/api/accounts/{provider}/{id}", delete(remove_account))
        .route("/api/logins/{id}", get(login_state).delete(close_login))
        .route("/api/logins/{id}/pty", get(login_pty))
}

/// 부를 때마다 데몬 PATH를 다시 훑는다. 화면의 [다시 찾기]는 이것을 다시 부른다.
async fn agents() -> Json<Vec<AgentView>> {
    Json(profile::agents())
}

async fn get_settings(State(app): State<Arc<App>>) -> Result<Json<SettingsView>, ApiError> {
    Ok(Json(app.settings.get().map_err(internal)?))
}

async fn put_settings(
    State(app): State<Arc<App>>,
    Json(view): Json<SettingsView>,
) -> Result<Json<()>, ApiError> {
    app.settings.put(&view).map_err(internal)?;
    Ok(Json(()))
}

/// 계정마다 사용량을 그 자리에서 읽는다. 공급자마다 시스템 기본이 첫째다.
async fn accounts(State(app): State<Arc<App>>) -> Result<Json<Vec<AccountView>>, ApiError> {
    let mut rows = Vec::new();
    for provider in PROVIDERS {
        let ids = app.accounts.list(provider).map_err(internal)?;
        rows.extend(
            std::iter::once(None)
                .chain(ids.into_iter().map(Some))
                .map(|id| (provider, id)),
        );
    }
    let probes = rows.iter().map(|(provider, id)| {
        let provider = *provider;
        let dir = app.accounts.config_dir(provider, id.as_ref());
        tokio::task::spawn_blocking(move || usage::probe(provider, dir.as_deref()))
    });
    let usages = join_all(probes).await;
    let views = rows
        .into_iter()
        .zip(usages)
        .map(|((provider, id), usage)| AccountView {
            provider,
            id,
            usage: usage.unwrap_or_else(|e| Usage::failed(e.to_string())),
        })
        .collect();
    Ok(Json(views))
}

async fn remove_account(
    State(app): State<Arc<App>>,
    Path((provider, id)): Path<(Profile, AccountId)>,
) -> Result<Json<()>, ApiError> {
    app.accounts
        .remove(provider, &id)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(()))
}

/// 로그인 창을 띄운다. 새 계정이면 로그인이 실패하거나 창을 닫았을 때 만든 폴더를 지운다.
async fn login(
    State(app): State<Arc<App>>,
    Path(provider): Path<Profile>,
    Json(req): Json<Login>,
) -> Result<Json<LoginId>, ApiError> {
    let created = req.id.is_none();
    let id = match req.id {
        Some(id) => id,
        None => app.accounts.create(provider).map_err(internal)?,
    };
    let discard = {
        let app = app.clone();
        let id = id.clone();
        move || {
            if created {
                // 지우지 못하면 빈 폴더가 「다시 로그인 필요」로 목록에 남는다. 사용자가 지울 수 있다.
                let _ = app.accounts.remove(provider, &id);
            }
        }
    };
    let cmd = match app.accounts.login(provider, &id) {
        Ok(cmd) => cmd,
        Err(e) => {
            discard();
            return Err(internal(e));
        }
    };
    let login = LoginId(app.logins.next.fetch_add(1, Ordering::Relaxed));
    app.logins
        .states
        .lock()
        .unwrap()
        .insert(login, LoginState::Running);
    let on_exit = {
        let app = app.clone();
        let discard = discard.clone();
        move |code: u32| {
            let state = if code == 0 {
                LoginState::Succeeded
            } else {
                discard();
                LoginState::Failed
            };
            if let Some(slot) = app.logins.states.lock().unwrap().get_mut(&login) {
                *slot = state;
            }
        }
    };
    app.logins.host.spawn(login, cmd, on_exit).map_err(|e| {
        app.logins.states.lock().unwrap().remove(&login);
        discard();
        internal(e)
    })?;
    Ok(Json(login))
}

async fn login_state(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
) -> Result<Json<LoginState>, ApiError> {
    let states = app.logins.states.lock().unwrap();
    let state = states
        .get(&LoginId(id))
        .ok_or((StatusCode::NOT_FOUND, "없는 로그인 창이다.".into()))?;
    Ok(Json(*state))
}

/// 창을 닫는다. 로그인이 아직 돌면 끝내고, 새 계정이면 폴더를 지운다.
async fn close_login(State(app): State<Arc<App>>, Path(id): Path<u32>) -> Json<()> {
    let id = LoginId(id);
    app.logins.host.kill(id);
    app.logins.states.lock().unwrap().remove(&id);
    Json(())
}

async fn login_pty(
    ws: WebSocketUpgrade,
    Path(id): Path<u32>,
    State(app): State<Arc<App>>,
) -> Response {
    serve_pty(ws, app, |app| &app.logins.host, LoginId(id))
}
