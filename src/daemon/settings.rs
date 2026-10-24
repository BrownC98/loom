use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, Query, State};
use axum::response::{Html, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::nodes::serve_pty;
use super::origin::ViaRemote;
use super::usage_cache::Key;
use super::{App, Problem, internal};
use crate::accounts::{AccountId, Site};
use crate::node_host::{Exit, NodeHost};
use crate::profile::{self, AgentView, Profile, Provider, VersionMismatch};
use crate::protocol::LaunchError;
#[cfg(test)]
use crate::settings::{Language, Theme};
use crate::settings::{PatchProblem, SettingsView};
use crate::usage::{self, Usage};
use crate::worktree::{self, RepoProblem};
use crate::wsl::{self, Inside};

const PROVIDERS: [Provider; 2] = [Provider::Claude, Provider::Codex];

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LoginId(pub u32);

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

pub struct Logins {
    host: NodeHost<LoginId>,
    states: Mutex<HashMap<LoginId, LoginState>>,
    next: AtomicU32,
}

impl Logins {
    pub fn new(dir: std::path::PathBuf) -> Self {
        Self {
            host: NodeHost::new(dir),
            states: Mutex::default(),
            next: AtomicU32::default(),
        }
    }

    pub fn end_others(&self) {
        self.host.end_others();
    }
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AccountView {
    pub provider: Provider,
    /// 계정이 있는 WSL 배포판. 이 PC면 없다. 계정은 한 위치에만 속한다.
    pub distro: Option<String>,
    /// 없으면 그 위치의 시스템 기본(`~/.claude`·`~/.codex`)이다. 쓰는 계정은 설정(`SettingsView`)이 위치마다 정한다.
    pub id: Option<AccountId>,
    pub usage: Usage,
}

/// 한 위치의 Cursor CLI 로그인과 사용량. loom은 Cursor 계정을 여럿 두지 않고 바꾸지 않는다(M18 v2, Q666 A).
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CursorView {
    /// WSL 배포판. 이 PC면 없다.
    pub distro: Option<String>,
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
        .route("/api/settings/default", get(default_settings))
        .route("/api/accounts", get(accounts))
        .route("/api/usage/cursor", get(cursor_usage))
        .route("/api/accounts/{provider}", post(login))
        .route("/api/accounts/{provider}/logout", post(logout))
        .route("/api/accounts/{provider}/{id}", delete(remove_account))
        .route("/api/logins/{id}", get(login_state).delete(close_login))
        .route("/api/logins/{id}/pty", get(login_pty))
}

async fn agents(State(app): State<Arc<App>>) -> Json<Vec<AgentView>> {
    let sites = app.sites().await;
    let wsl: Vec<_> = sites.iter().filter_map(Site::inside).collect();
    let warnings = join_all(wsl.iter().map(|inside| wsl_version_warnings(inside))).await;
    let mut agents = profile::agents(&wsl);
    for agent in &mut agents {
        let Some(profile) = agent.profile else {
            continue;
        };
        for (i, place) in agent.places.iter_mut().enumerate() {
            let warnings = match i {
                0 => &app.version_warnings,
                _ => &warnings[i - 1],
            };
            place.version_warning = warnings.get(&profile).cloned();
        }
    }
    Json(agents)
}

async fn wsl_version_warnings(inside: &Inside) -> HashMap<Profile, VersionMismatch> {
    let reads = profile::PINNED.into_iter().filter_map(|(profile, cmd)| {
        let program = inside.find(&[cmd.to_owned()])?.to_owned();
        let mut cmd = tokio::process::Command::from(
            inside.server(&[program.as_str(), "--version"], &[] as &[(&str, &str)]),
        );
        cmd.stdin(std::process::Stdio::null());
        Some(async move {
            let out = cmd.output().await.ok()?;
            profile::version_warning(profile, &String::from_utf8_lossy(&out.stdout))
                .map(|w| (profile, w))
        })
    });
    join_all(reads).await.into_iter().flatten().collect()
}

async fn index(State(app): State<Arc<App>>) -> Result<Html<String>, Problem> {
    let settings = app.settings.get().map_err(internal)?;
    let page = super::Assets::get("index.html")
        .ok_or_else(|| internal("web/dist has no index.html. Run `npm run build`."))?;
    Ok(Html(with_settings(
        &String::from_utf8_lossy(&page.data),
        &settings,
    )))
}

fn with_settings(page: &str, settings: &SettingsView) -> String {
    let theme = serde_json::to_string(&settings.theme).expect("Theme은 늘 직렬화된다");
    let language = serde_json::to_string(&settings.language).expect("Language는 늘 직렬화된다");
    page.replacen(
        "<html",
        &format!("<html data-theme={theme} data-language={language}"),
        1,
    )
}

async fn get_settings(State(app): State<Arc<App>>) -> Result<Json<SettingsView>, Problem> {
    Ok(Json(shown(app.settings.get().map_err(internal)?)))
}

async fn default_settings() -> Json<SettingsView> {
    Json(shown(SettingsView::default()))
}

fn shown(mut view: SettingsView) -> SettingsView {
    if view.workspace_dir.is_none() {
        view.workspace_dir = worktree::default_workspace()
            .ok()
            .map(|p| p.to_string_lossy().into_owned());
    }
    view
}

async fn put_settings(
    State(app): State<Arc<App>>,
    Json(patch): Json<serde_json::Value>,
) -> Result<Json<()>, Problem> {
    let wsl_dirs = patch
        .get("wslWorkspaceDirs")
        .and_then(|dirs| dirs.as_object())
        .into_iter()
        .flat_map(|dirs| dirs.values());
    let dirs = patch.get("workspaceDir").into_iter().chain(wsl_dirs);
    if let Some(dir) = dirs
        .filter_map(|dir| dir.as_str())
        .find(|dir| !std::path::Path::new(dir).is_absolute())
    {
        return Err(Problem::Repo(RepoProblem::NotAbsolute(dir.to_owned())));
    }
    match app.settings.patch(&patch) {
        Ok(_) => {}
        Err(PatchProblem::Db(e)) => return Err(internal(e)),
        Err(PatchProblem::Shape(e)) => return Err(internal(e)),
    }
    app.graph.publish_view_changed();
    Ok(Json(()))
}

async fn accounts(State(app): State<Arc<App>>) -> Result<Json<Vec<AccountView>>, Problem> {
    let sites: Vec<_> = app.sites().await.into_iter().map(Arc::new).collect();
    let mut rows = Vec::new();
    for site in &sites {
        for provider in PROVIDERS {
            let ids = site.accounts().list(provider).map_err(internal)?;
            rows.extend(
                std::iter::once(None)
                    .chain(ids.into_iter().map(Some))
                    .map(|id| (site.clone(), provider, id)),
            );
        }
    }
    let probes = rows.iter().map(|(site, provider, id)| {
        let (site, provider, id) = (site.clone(), *provider, id.clone());
        let key = Key::provider(&site, provider, id.as_ref());
        let fingerprint = usage::credentials_fingerprint(provider, &site, id.as_ref());
        app.usage.get(key, fingerprint, move || {
            usage::probe(provider, &site, id.as_ref())
        })
    });
    let usages = join_all(probes).await;
    let views = rows
        .into_iter()
        .zip(usages)
        .map(|((site, provider, id), usage)| AccountView {
            provider,
            distro: site.distro().map(str::to_owned),
            id,
            usage,
        })
        .collect();
    Ok(Json(views))
}

async fn cursor_usage(State(app): State<Arc<App>>) -> Json<Vec<CursorView>> {
    let sites = app.sites().await;
    let probes = sites.into_iter().map(|site| {
        let app = app.clone();
        async move {
            let distro = site.distro().map(str::to_owned);
            let key = Key::cursor(&site);
            let fingerprint = usage::cursor_credentials_fingerprint(&site);
            let usage = app
                .usage
                .get(key, fingerprint, move || usage::probe_cursor(&site))
                .await;
            CursorView { distro, usage }
        }
    });
    Json(join_all(probes).await)
}

#[derive(Deserialize)]
struct At {
    distro: Option<String>,
}

async fn remove_account(
    State(app): State<Arc<App>>,
    Path((provider, id)): Path<(Provider, AccountId)>,
    Query(at): Query<At>,
) -> Result<Json<()>, Problem> {
    let site = app.site_of(at.distro.as_deref()).await?;
    site.accounts().remove(provider, &id).map_err(internal)?;
    Ok(Json(()))
}

async fn login(
    State(app): State<Arc<App>>,
    Path(provider): Path<Provider>,
    Query(at): Query<At>,
    Json(target): Json<LoginTarget>,
) -> Result<Json<LoginId>, Problem> {
    let site = Arc::new(app.site_of(at.distro.as_deref()).await?);
    site.login(provider, None)?;
    let (id, created) = match target {
        LoginTarget::System => (None, false),
        LoginTarget::Account(id) => (Some(id), false),
        LoginTarget::New => (
            Some(site.accounts().create(provider).map_err(internal)?),
            true,
        ),
    };
    if let Some(id) = &id
        && site.accounts().config_dir(provider, Some(id)).is_none()
    {
        return Err(internal("the account folder is gone"));
    }
    let cmd = site.login(provider, id.as_ref())?;
    let discard = {
        let site = site.clone();
        move || {
            if created && let Some(id) = &id {
                let _ = site.accounts().remove(provider, id);
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
        .spawn(login, cmd, None, on_exit)
        .map_err(|e| {
            app.logins.states.lock().unwrap().remove(&login);
            discard();
            internal(e)
        })?;
    Ok(Json(login))
}

async fn logout(
    State(app): State<Arc<App>>,
    Path(provider): Path<Provider>,
    Query(at): Query<At>,
) -> Result<Json<()>, Problem> {
    let mut cmd = app.site_of(at.distro.as_deref()).await?.logout(provider)?;
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

impl App {
    pub(super) fn pc_site(&self) -> Site {
        Site::pc(self.accounts_root.clone())
    }

    pub(super) async fn sites(&self) -> Vec<Site> {
        let running = wsl::running().await;
        let mut found: BTreeMap<String, wsl::WslPath> = BTreeMap::new();
        for (_, path) in self.repos.list().unwrap_or_default() {
            if let Some(at) = wsl::of(&path)
                && running.contains(&at.distro)
            {
                found.entry(at.distro.clone()).or_insert(at);
            }
        }
        let commands = profile::catalog_commands();
        let read = found
            .values()
            .map(|at| wsl::inside(at, &commands, &self.shim, &[]));
        let mut sites = vec![self.pc_site()];
        for inside in join_all(read).await {
            match inside {
                Ok(inside) => sites.push(Site::wsl(inside)),
                Err(e) => eprintln!("loom: WSL 위치를 읽지 못했다: {e}"),
            }
        }
        sites
    }

    pub(super) async fn site_of(&self, distro: Option<&str>) -> Result<Site, Problem> {
        let Some(distro) = distro else {
            return Ok(self.pc_site());
        };
        let repos = self.repos.list().map_err(internal)?;
        let path = repos
            .iter()
            .map(|(_, path)| path)
            .find(|path| wsl::of(path).is_some_and(|at| at.distro == distro))
            .ok_or_else(|| internal(format!("no repository in WSL · {distro}")))?;
        self.site_at(path).await
    }

    pub(super) async fn site_at(&self, cwd: &std::path::Path) -> Result<Site, Problem> {
        Ok(match wsl::of(cwd) {
            Some(at) => Site::wsl(
                wsl::inside(&at, &["claude", "codex"], &self.shim, &[])
                    .await
                    .map_err(LaunchError::Wsl)?,
            ),
            None => self.pc_site(),
        })
    }
}

async fn login_state(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
) -> Result<Json<LoginState>, Problem> {
    let states = app.logins.states.lock().unwrap();
    let state = states.get(&LoginId(id)).ok_or(Problem::NoLogin)?;
    Ok(Json(*state))
}

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
    via: Option<Extension<ViaRemote>>,
) -> Response {
    serve_pty(ws, via, app, |app| &app.logins.host, |_, _| {}, LoginId(id))
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
