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
use super::{App, Problem, internal};
use crate::accounts::{AccountId, Site};
use crate::distro::Host;
use crate::node_host::{Exit, NodeHost};
use crate::place::{self, Machine};
use crate::profile::{self, Accounted, AgentView, Profile, VersionMismatch};
use crate::protocol::{LaunchError, Provider};
use crate::settings::{PatchProblem, SettingsView};
use crate::usage::Usage;
use crate::worktree::{self, RepoProblem};
use crate::wsl::{self, Inside};

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
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AccountView {
    pub provider: Provider,
    /// 계정이 있는 WSL 배포판. 이 PC면 없다. 계정은 한 위치에만 속한다.
    pub distro: Option<String>,
    /// 없으면 그 위치의 시스템 기본(그 CLI가 `HOME` 아래에서 고르는 로그인 폴더)이다. 쓰는 계정은 설정(`SettingsView`)이 위치마다 정한다.
    pub id: Option<AccountId>,
    pub usage: Usage,
}

/// 한 위치의 CLI 로그인 하나와 그 사용량. loom은 이 로그인을 여럿 두지 않고 바꾸지 않는다(M18 v2, Q666 A).
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlaceUsage {
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
    /// 시스템 기본(그 CLI가 `HOME` 아래에서 고르는 로그인 폴더)에 로그인한다.
    System,
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/", get(index))
        .route("/api/agents", get(agents))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/settings/default", get(default_settings))
        .route("/api/accounts", get(accounts))
        .route("/api/usage/{profile}", get(place_usage))
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
    let pc = profile::version_warnings(&app.versions);
    let mut agents = profile::agents(&wsl);
    agents.sort_by_cached_key(|a| a.label.to_lowercase());
    for agent in &mut agents {
        let profile = agent.profile;
        for (i, place) in agent.places.iter_mut().enumerate() {
            let warnings = match i {
                0 => &pc,
                _ => &warnings[i - 1],
            };
            place.version_warning = warnings.get(&profile).cloned();
        }
    }
    Json(agents)
}

async fn wsl_version_warnings(inside: &Inside) -> HashMap<Profile, VersionMismatch> {
    let reads = profile::pinned().into_iter().filter_map(|(profile, cmd)| {
        let program = inside.find(&[cmd.to_owned()])?;
        let run = inside.run(&[program, "--version"], &[] as &[(&str, &str)]);
        Some(async move {
            let ran = tokio::task::spawn_blocking(move || run.output(crate::usage::PROBE_TIMEOUT))
                .await
                .ok()?
                .ok()?;
            profile::version_warning(profile, &ran.stdout).map(|w| (profile, w))
        })
    });
    join_all(reads).await.into_iter().flatten().collect()
}

pub(super) async fn index(State(app): State<Arc<App>>) -> Result<Html<String>, Problem> {
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
        for accounted in profile::with_accounts() {
            let ids = site.accounts().list(accounted.cli).map_err(internal)?;
            rows.extend(
                std::iter::once(None)
                    .chain(ids.into_iter().map(Some))
                    .map(|id| (site.clone(), accounted, id)),
            );
        }
    }
    let probes = rows
        .iter()
        .map(|(site, a, id)| app.usage.read(site, a.profile, a.meter, id.clone()));
    let usages = join_all(probes).await;
    let views = rows
        .into_iter()
        .zip(usages)
        .map(|((site, a, id), usage)| AccountView {
            provider: a.cli.provider,
            distro: site.distro().map(str::to_owned),
            id,
            usage,
        })
        .collect();
    Ok(Json(views))
}

async fn place_usage(
    State(app): State<Arc<App>>,
    Path(profile): Path<Profile>,
) -> Json<Vec<PlaceUsage>> {
    let Some(meter) = profile.meter() else {
        return Json(Vec::new());
    };
    let sites: Vec<_> = app.sites().await.into_iter().map(Arc::new).collect();
    let probes = sites.iter().map(|site| async {
        PlaceUsage {
            distro: site.distro().map(str::to_owned),
            usage: app.usage.read(site, profile, meter, None).await,
        }
    });
    Json(join_all(probes).await)
}

fn accounted(provider: Provider) -> Result<Accounted, Problem> {
    profile::of_provider(provider).ok_or_else(|| internal("no agent adapter has these accounts"))
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
    let cli = accounted(provider)?.cli;
    let site = app.site_of(at.distro.as_deref()).await?;
    site.accounts().remove(cli, &id).map_err(internal)?;
    Ok(Json(()))
}

async fn login(
    State(app): State<Arc<App>>,
    Path(provider): Path<Provider>,
    Query(at): Query<At>,
    Json(target): Json<LoginTarget>,
) -> Result<Json<LoginId>, Problem> {
    let cli = accounted(provider)?.cli;
    let site = Arc::new(app.site_of(at.distro.as_deref()).await?);
    site.login(cli, None)?;
    let (id, created) = match target {
        LoginTarget::System => (None, false),
        LoginTarget::Account(id) => (Some(id), false),
        LoginTarget::New => (Some(site.accounts().create(cli).map_err(internal)?), true),
    };
    if let Some(id) = &id
        && site.accounts().config_dir(cli, Some(id)).is_none()
    {
        return Err(internal("the account folder is gone"));
    }
    let cmd = site.login(cli, id.as_ref())?;
    let discard = {
        let site = site.clone();
        move || {
            if created && let Some(id) = &id {
                let _ = site.accounts().remove(cli, id);
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
            app.graph.publish_view_changed();
        }
    };
    app.logins
        .host
        .spawn_while_linked(login, &cmd, on_exit)
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
    let cli = accounted(provider)?.cli;
    let mut cmd = app.site_of(at.distro.as_deref()).await?.logout(cli)?;
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
            match place::of(&path) {
                Machine::Wsl(at) if running.contains(&at.distro) => {
                    found.entry(at.distro.clone()).or_insert(at);
                }
                Machine::Pc | Machine::Wsl(_) | Machine::Ssh(_) => {}
            }
        }
        let commands = profile::catalog_commands();
        let read = found.values().map(|at| async {
            let distro = self.host(Host::Distro(at.distro.clone()))?;
            wsl::inside(at, distro, &commands, &self.shim, &[]).await
        });
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
            .find(|path| matches!(place::of(path), Machine::Wsl(at) if at.distro == distro))
            .ok_or_else(|| internal(format!("no repository in WSL · {distro}")))?;
        self.site_at(path).await
    }

    pub(super) async fn site_at(&self, cwd: &std::path::Path) -> Result<Site, Problem> {
        Ok(match place::of(cwd) {
            Machine::Wsl(at) => {
                let distro = self
                    .host(Host::Distro(at.distro.clone()))
                    .map_err(LaunchError::Wsl)?;
                let commands: Vec<_> = profile::with_accounts().map(|a| a.cli.command).collect();
                let inside = wsl::inside(&at, distro, &commands, &self.shim, &[]);
                Site::wsl(inside.await.map_err(LaunchError::Wsl)?)
            }
            Machine::Pc => self.pc_site(),
            Machine::Ssh(at) => {
                let (distro, remote) = self.ssh_host(&at).await.map_err(LaunchError::Wsl)?;
                let commands: Vec<_> = profile::with_accounts().map(|a| a.cli.command).collect();
                let inside =
                    wsl::inside_ssh(at.url(""), &at.path, distro, &remote, &commands, Vec::new());
                Site::wsl(inside.await.map_err(LaunchError::Wsl)?)
            }
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
    serve_pty(
        ws,
        via,
        app,
        |app| &app.logins.host,
        |_, _, _| {},
        LoginId(id),
    )
}
