//! 설정 창과 상태 줄의 라우트: 에이전트 탐지, 설정, 계정, 사용량, 로그인 창, 테마를 넣은 첫 페이지.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, State};
use axum::response::{Html, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{App, Problem, internal, serve_pty};
use crate::accounts::{self, AccountId};
use crate::node_host::{Exit, NodeHost};
use crate::profile::{self, AgentView, Provider};
use crate::settings::SettingsView;
#[cfg(test)]
use crate::settings::{Language, Theme};
use crate::usage::{self, Usage, UsageProblem};
use crate::worktree::{self, RepoProblem};

const PROVIDERS: [Provider; 2] = [Provider::Claude, Provider::Codex];

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LoginId(pub u32);

/// PTY 호스트 주소 파일 이름.
impl std::fmt::Display for LoginId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Copy, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum LoginState {
    Running,
    Succeeded,
    Failed,
}

/// 계정 추가·다시 로그인 창의 PTY. 그래프에는 나타나지 않는다.
/// 창 번호는 데몬마다 0부터 다시 센다. 그래서 앞 데몬의 로그인 창은 다시 붙지 않고 끝낸다(`end_others`).
pub struct Logins {
    host: NodeHost<LoginId>,
    states: Mutex<HashMap<LoginId, LoginState>>,
    next: AtomicU32,
}

impl Logins {
    /// `dir`는 로그인 창 PTY 호스트의 주소 파일 폴더다.
    pub fn new(dir: std::path::PathBuf) -> Self {
        Self {
            host: NodeHost::new(dir),
            states: Mutex::default(),
            next: AtomicU32::default(),
        }
    }

    /// 앞 데몬이 띄운 로그인 창을 끝낸다. 데몬이 시작할 때 부른다.
    pub fn end_others(&self) {
        self.host.end_others();
    }
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AccountView {
    pub provider: Provider,
    /// 없으면 시스템 기본(`~/.claude`·`~/.codex`)이다. 쓰는 계정은 설정(`SettingsView`)이 정한다.
    pub id: Option<AccountId>,
    pub usage: Usage,
}

/// 로그인 창이 로그인할 곳.
#[derive(Deserialize, TS)]
#[serde(tag = "kind", content = "id", rename_all = "camelCase")]
#[ts(export)]
pub enum LoginTarget {
    /// loom 계정을 새로 만든다. 로그인이 실패하거나 창을 닫으면 만든 폴더를 지운다.
    New,
    /// 그 loom 계정에 다시 로그인한다.
    Account(AccountId),
    /// 시스템 기본(`~/.claude`·`~/.codex`)에 로그인한다.
    System,
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/", get(index))
        .route("/api/agents", get(agents))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/accounts", get(accounts))
        .route("/api/accounts/{provider}", post(login))
        .route("/api/accounts/{provider}/logout", post(logout))
        .route("/api/accounts/{provider}/{id}", delete(remove_account))
        .route("/api/logins/{id}", get(login_state).delete(close_login))
        .route("/api/logins/{id}/pty", get(login_pty))
}

/// 부를 때마다 데몬 PATH를 다시 훑는다. 화면의 [다시 찾기]는 이것을 다시 부른다.
/// 서버를 쓰는 프로필(Codex, OpenCode 2) 줄에는 데몬이 시작할 때 본 버전 경고를 붙인다.
async fn agents(State(app): State<Arc<App>>) -> Json<Vec<AgentView>> {
    let mut agents = profile::agents();
    for agent in &mut agents {
        agent.version_warning = agent
            .profile
            .and_then(|p| app.version_warnings.get(&p).cloned());
    }
    Json(agents)
}

/// 첫 그리기 전에 테마와 언어를 정한다. 설정 값을 `<html data-theme data-language>`에 넣어 보낸다.
/// 화면 스크립트가 돌기 전이라 다른 테마·언어가 잠깐 보이지 않는다.
async fn index(State(app): State<Arc<App>>) -> Result<Html<String>, Problem> {
    let settings = app.settings.get().map_err(internal)?;
    let page = super::Assets::get("index.html")
        .ok_or_else(|| internal("web/dist has no index.html. Run `npm run build`."))?;
    Ok(Html(with_settings(
        &String::from_utf8_lossy(&page.data),
        &settings,
    )))
}

/// 속성 값은 serde 이름이다. 화면 타입 `Theme`·`Language`(gen/)와 같은 글자다.
fn with_settings(page: &str, settings: &SettingsView) -> String {
    let theme = serde_json::to_string(&settings.theme).expect("Theme은 늘 직렬화된다");
    let language = serde_json::to_string(&settings.language).expect("Language는 늘 직렬화된다");
    page.replacen(
        "<html",
        &format!("<html data-theme={theme} data-language={language}"),
        1,
    )
}

/// 워크스페이스 폴더가 비어 있으면 기본값을 채워 보낸다. 화면은 늘 실제로 쓸 폴더를 보인다.
async fn get_settings(State(app): State<Arc<App>>) -> Result<Json<SettingsView>, Problem> {
    let mut view = app.settings.get().map_err(internal)?;
    if view.workspace_dir.is_none() {
        view.workspace_dir = worktree::default_workspace()
            .ok()
            .map(|p| p.to_string_lossy().into_owned());
    }
    Ok(Json(view))
}

async fn put_settings(
    State(app): State<Arc<App>>,
    Json(view): Json<SettingsView>,
) -> Result<Json<()>, Problem> {
    if let Some(dir) = &view.workspace_dir
        && !std::path::Path::new(dir).is_absolute()
    {
        return Err(Problem::Repo(RepoProblem::NotAbsolute(dir.clone())));
    }
    app.settings.put(&view).map_err(internal)?;
    app.graph.publish_view_changed();
    Ok(Json(()))
}

/// 계정마다 사용량을 그 자리에서 읽는다. 공급자마다 시스템 기본이 첫째다.
async fn accounts(State(app): State<Arc<App>>) -> Result<Json<Vec<AccountView>>, Problem> {
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
            usage: usage
                .unwrap_or_else(|e| Usage::failed(UsageProblem::Unavailable(e.to_string()))),
        })
        .collect();
    Ok(Json(views))
}

async fn remove_account(
    State(app): State<Arc<App>>,
    Path((provider, id)): Path<(Provider, AccountId)>,
) -> Result<Json<()>, Problem> {
    app.accounts.remove(provider, &id).map_err(internal)?;
    Ok(Json(()))
}

/// 로그인 창을 띄운다. 새 계정이면 로그인이 실패하거나 창을 닫았을 때 만든 폴더를 지운다.
async fn login(
    State(app): State<Arc<App>>,
    Path(provider): Path<Provider>,
    Json(target): Json<LoginTarget>,
) -> Result<Json<LoginId>, Problem> {
    // CLI가 없으면 폴더를 만들기 전에 거절한다.
    let mut cmd = accounts::login(provider)?;
    let (id, created) = match target {
        LoginTarget::System => (None, false),
        LoginTarget::Account(id) => (Some(id), false),
        LoginTarget::New => (Some(app.accounts.create(provider).map_err(internal)?), true),
    };
    if let Some(id) = &id {
        // `apply`는 폴더가 없으면 시스템 기본으로 둔다. 사라진 계정은 시스템 기본에 로그인하지 않게 거절한다.
        if app.accounts.config_dir(provider, Some(id)).is_none() {
            return Err(internal("the account folder is gone"));
        }
        app.accounts.apply(&mut cmd, provider, Some(id));
    }
    let discard = {
        let app = app.clone();
        move || {
            if created && let Some(id) = &id {
                // 지우지 못하면 빈 폴더가 「다시 로그인 필요」로 목록에 남는다. 사용자가 지울 수 있다.
                let _ = app.accounts.remove(provider, id);
            }
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
        move |exit| {
            let state = if exit == Exit::Command(0) {
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
    app.logins
        .host
        .spawn(login, cmd, None, |_| {}, on_exit)
        .map_err(|e| {
            app.logins.states.lock().unwrap().remove(&login);
            discard();
            internal(e)
        })?;
    Ok(Json(login))
}

/// 시스템 기본의 로그인을 지운다. CLI가 끝날 때까지 기다린다.
async fn logout(Path(provider): Path<Provider>) -> Result<Json<()>, Problem> {
    let mut cmd = accounts::logout(provider)?;
    let out = tokio::task::spawn_blocking(move || cmd.stdin(std::process::Stdio::null()).output())
        .await
        .map_err(internal)?
        .map_err(internal)?;
    if !out.status.success() {
        return Err(internal(
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        ));
    }
    Ok(Json(()))
}

async fn login_state(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
) -> Result<Json<LoginState>, Problem> {
    let states = app.logins.states.lock().unwrap();
    let state = states.get(&LoginId(id)).ok_or(Problem::NoLogin)?;
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
    serve_pty(ws, app, |app| &app.logins.host, |_, _| {}, LoginId(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_settings_marks_html() {
        let settings = SettingsView {
            theme: Theme::Light,
            language: Language::Ko,
            ..Default::default()
        };
        let page = with_settings("<!doctype html>\n<html lang=\"en\">\n</html>", &settings);
        assert_eq!(
            page,
            "<!doctype html>\n<html data-theme=\"light\" data-language=\"ko\" lang=\"en\">\n</html>"
        );
    }
}
