//! 설정 창과 상태 줄의 라우트: 에이전트 탐지, 설정, 계정, 사용량, 로그인 창, 테마를 넣은 첫 페이지.

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

use super::origin::ViaRemote;
use super::usage_cache::Key;
use super::{App, Problem, internal, serve_pty};
use crate::accounts::{AccountId, Site};
use crate::node_host::{Exit, NodeHost};
use crate::profile::{self, AgentView, LaunchError, Profile, Provider, VersionMismatch};
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

/// 부를 때마다 데몬 PATH와 도는 WSL 배포판의 PATH를 다시 훑는다. 화면의 [다시 찾기]는 이것을 다시 부른다.
/// 서버를 쓰는 프로필(Codex, OpenCode 2) 줄에는 버전 경고를 붙인다. 이 PC는 데몬이 시작할 때 본 값, WSL은 지금 읽은 값이다.
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

/// 배포판 안의 고정 버전 CLI(`PINNED`)마다 `--version`을 읽어 경고를 만든다. 없는 CLI는 건너뛴다.
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
    Ok(Json(shown(app.settings.get().map_err(internal)?)))
}

/// 모든 칸이 기본값인 설정. 화면이 칸마다 [기본값] 단추를 보일지 견준다. 기본값은 여기(Rust)에만 있다.
async fn default_settings() -> Json<SettingsView> {
    Json(shown(SettingsView::default()))
}

/// 화면에 보이는 설정. 작업 폴더가 비면 기본 작업 폴더를 넣는다.
fn shown(mut view: SettingsView) -> SettingsView {
    if view.workspace_dir.is_none() {
        view.workspace_dir = worktree::default_workspace()
            .ok()
            .map(|p| p.to_string_lossy().into_owned());
    }
    view
}

/// 화면이 바꾼 칸만 보낸다(`Settings::patch`). 합친 설정이 규칙에 맞지 않으면 쓰지 않는다.
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
    // 모양이 틀린 칸은 화면의 잘못이다. 사람이 고칠 수 없으므로 내부 오류로 알린다.
    match app.settings.patch(&patch) {
        Ok(_) => {}
        Err(PatchProblem::Db(e)) => return Err(internal(e)),
        Err(PatchProblem::Shape(e)) => return Err(internal(e)),
    }
    app.graph.publish_view_changed();
    Ok(Json(()))
}

/// 위치(이 PC, 도는 WSL 배포판)와 계정마다 사용량을 그 자리에서 읽는다. 위치마다, 공급자마다 시스템 기본이 첫째다.
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
        app.usage
            .get(key, move || usage::probe(provider, &site, id.as_ref()))
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

/// 위치마다 Cursor 사용량을 그 자리에서 읽는다. `accounts`와 같은 때 화면이 부른다.
async fn cursor_usage(State(app): State<Arc<App>>) -> Json<Vec<CursorView>> {
    let sites = app.sites().await;
    let probes = sites.into_iter().map(|site| {
        let app = app.clone();
        async move {
            let distro = site.distro().map(str::to_owned);
            let key = Key::cursor(&site);
            let usage = app.usage.get(key, move || usage::probe_cursor(&site)).await;
            CursorView { distro, usage }
        }
    });
    Json(join_all(probes).await)
}

/// 계정 라우트의 위치. `distro`가 없으면 이 PC다.
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
    app.usage.forget(&Key::provider(&site, provider, Some(&id)));
    Ok(Json(()))
}

/// 로그인 창을 띄운다. 새 계정이면 로그인이 실패하거나 창을 닫았을 때 만든 폴더를 지운다.
/// 그 위치의 CLI로 로그인하고, 새 계정의 폴더도 그 위치에 만든다.
async fn login(
    State(app): State<Arc<App>>,
    Path(provider): Path<Provider>,
    Query(at): Query<At>,
    Json(target): Json<LoginTarget>,
) -> Result<Json<LoginId>, Problem> {
    let site = Arc::new(app.site_of(at.distro.as_deref()).await?);
    // CLI가 없으면 폴더를 만들기 전에 거절한다.
    site.login(provider, None)?;
    let (id, created) = match target {
        LoginTarget::System => (None, false),
        LoginTarget::Account(id) => (Some(id), false),
        LoginTarget::New => (
            Some(site.accounts().create(provider).map_err(internal)?),
            true,
        ),
    };
    // `login`은 폴더가 없으면 시스템 기본으로 둔다. 사라진 계정은 시스템 기본에 로그인하지 않게 거절한다.
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
                // 지우지 못하면 빈 폴더가 「다시 로그인 필요」로 목록에 남는다. 사용자가 지울 수 있다.
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

/// 그 위치의 시스템 기본 로그인을 지운다. CLI가 끝날 때까지 기다린다.
async fn logout(
    State(app): State<Arc<App>>,
    Path(provider): Path<Provider>,
    Query(at): Query<At>,
) -> Result<Json<()>, Problem> {
    let site = app.site_of(at.distro.as_deref()).await?;
    let mut cmd = site.logout(provider)?;
    let out = tokio::task::spawn_blocking(move || cmd.stdin(std::process::Stdio::null()).output())
        .await
        .map_err(internal)?
        .map_err(internal)?;
    if !out.status.success() {
        return Err(internal(
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        ));
    }
    app.usage.forget(&Key::provider(&site, provider, None));
    Ok(Json(()))
}

impl App {
    pub(super) fn pc_site(&self) -> Site {
        Site::pc(self.accounts_root.clone())
    }

    /// 위치 목록: 이 PC, 그리고 더한 저장소가 있고 지금 도는 WSL 배포판. 멈춘 배포판은 켜지 않는다(`wsl::running`).
    /// 배포판마다 `wsl.exe` 한 번으로 카탈로그 명령을 모두 찾는다. 읽지 못한 배포판은 빠진다.
    /// 위치가 하나면(Linux, macOS, WSL 저장소가 없는 Windows) 화면은 위치를 보이지 않는다.
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

    /// 화면이 고른 위치. 배포판이면 그 배포판의 저장소 하나에서 읽는다. 꺼져 있으면 `wsl.exe`가 켠다.
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

    /// 노드 폴더 `cwd`의 위치. 그 위치의 계정과 CLI(`claude`, `codex`)를 쓴다.
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
