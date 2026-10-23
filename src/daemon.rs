mod carry;
pub mod gate;
mod idle;
mod lan;
mod origin;
mod passes;
mod phone;
mod places;
mod relaunch;
mod sessions;
mod settings;
mod sleep;
pub use relaunch::{NewAgent, Relaunched};
mod switch;
mod turn_end;
mod usage_cache;
#[cfg(test)]
mod verb_traces;
mod verbs;

use origin::ViaRemote;

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::future::IntoFuture;
use std::hash::Hash;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use rust_embed::Embed;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;
use ts_rs::TS;

use crate::accounts::{AccountId, Site};
use crate::codex_server::CodexServer;
use crate::commit_graph::{self, Commit};
use crate::db;
use crate::devices::Devices;
use crate::graph::{
    CardView, Delivery, EventRecord, Graph, GraphEvent, IMAGE_EXTENSIONS, IdleEnd, Kind, Launched,
    Listed, MessageId, Model, NewChild, NodeId, PARENT, Place, Prompt, REDELIVER_AFTER, Refusal,
    Relations, Report, Sent, TreeLimits, Via, WaitDoc, WaitId, WaitKind, WaitSource,
};
use crate::launch;
use crate::node_host::{self, Exit, NodeHost};
use crate::open;
use crate::paste;
use crate::process_tree;
use crate::profile::claude;
use crate::profile::codex::{self, Status};
use crate::profile::opencode2;
use crate::profile::{
    self, AgentId, LaunchError, Mailbox, ModelChoice, Profile, Remote, ServerLink, Start,
    VersionMismatch,
};
use crate::protocol::{MessageKind, Question};
use crate::roles::{Assignment, RoleProblem, Roles};
use crate::screen;
use crate::settings::Settings;
use crate::skills::{self, Launch, Position};
use crate::spots::{Arranged, CardSize, Layout, Placed, Spots};
use crate::task;
use crate::update::{UpdateView, Updater};
use crate::user_doc;
use crate::worktree::{self, FolderList, RepoId, RepoProblem, RepoView, Repos, Worktree};
use crate::wsl;
use verbs::{
    Attempt, agent_adopt, agent_answer, agent_answered, agent_ask, agent_done, agent_note,
    agent_remove, agent_spawn, traced,
};

#[derive(Embed)]
#[folder = "web/dist"]
struct Assets;

struct App {
    database: db::Database,
    repos: Repos,
    host: NodeHost<NodeId>,
    graph: Arc<Graph>,
    settings: Settings,
    spots: Spots,
    accounts_root: PathBuf,
    logins: settings::Logins,
    codex: Mutex<HashMap<NodeId, Arc<CodexServer>>>,
    opencode2: Mutex<HashMap<NodeId, Arc<opencode2::Node>>>,
    sessions: sessions::Sessions,
    first_inputs: sessions::FirstInputs,
    setups: Mutex<HashSet<NodeId>>,
    idle: idle::IdleWaits,
    relaunches: relaunch::Relaunches,
    sleeps: sleep::Sleeps,
    switch: switch::Switches,
    usage: usage_cache::UsageCache,
    turn_ends: turn_end::TurnEnds,
    plugin: skills::Plugin,
    shim: PathBuf,
    homes: wsl::Homes,
    version_warnings: HashMap<Profile, VersionMismatch>,
    codex_config: Vec<String>,
    url: String,
    spawn_url: String,
    path: OsString,
    update: Arc<Updater>,
    devices: Arc<Devices>,
    port: u16,
    door: lan::Door,
    passes: passes::Passes,
}

/// 화면 경로가 실패하면 보내는 몸통(JSON). 화면이 고른 언어로 쓴다.
#[derive(Debug, Serialize, TS)]
#[serde(tag = "code", content = "value", rename_all = "camelCase")]
#[ts(export)]
pub enum Problem {
    Refused(Refusal),
    Launch(LaunchError),
    Repo(RepoProblem),
    /// 없는 로그인 창이다.
    NoLogin,
    /// DB·파일 오류처럼 요청을 고쳐도 낫지 않는 실패. 값은 원인 글이다.
    Internal(String),
}

impl Problem {
    fn status(&self) -> StatusCode {
        match self {
            Problem::Refused(Refusal::Db(_)) | Problem::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
            Problem::Refused(_) | Problem::Launch(_) | Problem::Repo(_) => StatusCode::BAD_REQUEST,
            Problem::NoLogin => StatusCode::NOT_FOUND,
        }
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        (self.status(), Json(self)).into_response()
    }
}

impl From<Refusal> for Problem {
    fn from(e: Refusal) -> Self {
        Problem::Refused(e)
    }
}

impl From<RepoProblem> for Problem {
    fn from(e: RepoProblem) -> Self {
        Problem::Repo(e)
    }
}

impl From<LaunchError> for Problem {
    fn from(e: LaunchError) -> Self {
        Problem::Launch(e)
    }
}

type AgentError = (StatusCode, String);

fn for_agent(e: impl Into<Problem>) -> AgentError {
    let problem = e.into();
    let text = match &problem {
        Problem::Refused(e) => e.to_string(),
        Problem::Launch(e) => e.to_string(),
        Problem::Repo(e) => e.to_string(),
        Problem::NoLogin => "No such login window.".into(),
        Problem::Internal(e) => e.clone(),
    };
    (problem.status(), text)
}

#[derive(Deserialize, TS)]
#[ts(export)]
pub struct NewNode {
    pub start: Start,
    pub place: Place,
}

/// 노드 이름 바꾸기. 이름 칸을 두 번 누르거나 F2를 눌러 고친다.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct RenameNode {
    pub name: String,
}

/// 터미널의 파일 링크를 누른다. `path`는 데몬이 보는 절대 경로다.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct OpenFile {
    pub path: String,
    /// 링크를 누른 터미널의 노드. 로그인 창이면 없다.
    #[serde(default)]
    pub node: Option<NodeId>,
}

#[derive(Deserialize)]
pub struct PasteAt {
    node: Option<NodeId>,
}

/// [저장소 추가]. `path`는 데몬이 보는 폴더다.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct NewRepo {
    pub path: String,
}

/// [저장소 추가] 「URL 복제」. `parent`는 데몬이 보는 폴더다.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct CloneRepo {
    pub url: String,
    pub parent: String,
}

/// [저장소 추가] 「새 프로젝트」. `<parent>/<name>`을 만든다.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct CreateRepo {
    pub parent: String,
    pub name: String,
}

#[derive(Deserialize)]
pub struct FolderQuery {
    pub path: Option<String>,
}

/// [새 워크트리]. `base`에서 새 브랜치 `branch`를 만든다.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct NewWorktree {
    pub branch: String,
    pub base: String,
}

#[derive(Deserialize)]
pub struct WorktreePath {
    pub path: String,
}

/// [이름 바꾸기]. `path` 워크트리의 브랜치 이름을 `name`으로 바꾼다.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct RenameWorktree {
    pub path: String,
    pub name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteWorktree {
    pub path: String,
    #[serde(default)]
    pub delete_branch: bool,
}

/// 커밋 그래프 칸이 읽는 것: 한 구간의 커밋과, 워크트리마다 체크아웃한 커밋.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct CommitGraph {
    pub commits: Vec<Commit>,
    /// 참조의 지문(`commit_graph::version`). 같으면 이미 읽은 구간도 그대로다.
    pub version: String,
    pub worktrees: Vec<Worktree>,
}

/// PTY 크기. 데몬은 바뀐 크기를 모든 화면에 알린다(`PtyNotice::Size`).
#[derive(Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Resize {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Serialize, Deserialize)]
pub struct SpawnRequest {
    pub name: String,
    pub run: SpawnRun,
    pub brief: String,
    pub worktree: Option<String>,
    #[serde(default)]
    pub choice: ModelChoice,
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct TaskRequest {
    pub branch: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SpawnRun {
    Agent(Option<AgentId>),
    Command(String),
}

#[derive(Serialize, Deserialize)]
pub struct NoteRequest {
    pub to: String,
    pub mode: Kind,
    pub body: String,
}

#[derive(Serialize, Deserialize)]
pub struct AnswerRequest {
    pub message: MessageId,
    pub body: String,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WaitRequest {
    pub reply_to: Option<MessageId>,
    pub timeout_secs: Option<u64>,
}

#[derive(Serialize, Deserialize)]
pub struct TakeRequest {
    pub hook: String,
    pub kinds: Vec<Kind>,
    pub limit_bytes: usize,
    #[serde(default)]
    pub transcript: Option<String>,
    #[serde(default)]
    pub human_input: bool,
    #[serde(default)]
    pub prompt: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct Taken {
    pub messages: Vec<Delivery>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct IdleRequest {
    pub transcript: Option<String>,
    pub rate_limited: bool,
}

#[derive(Serialize, Deserialize)]
pub struct GuardRequest {
    pub caller: u32,
}

#[derive(Serialize, Deserialize)]
pub struct AckRequest {
    pub ids: Vec<MessageId>,
}

#[derive(Serialize, Deserialize)]
pub struct AdoptRequest {
    pub name: String,
}

#[derive(Serialize, Deserialize)]
pub struct RemoveRequest {
    pub name: String,
}

#[derive(Serialize, Deserialize)]
pub struct RelaunchRequest {
    pub name: Option<String>,
    #[serde(default)]
    pub to: Option<NewAgent>,
}

#[derive(Serialize, Deserialize)]
pub struct AskRequest {
    pub question: String,
    #[serde(default)]
    pub doc: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AskResult {
    Inbox(Asked),
    Parent(Sent),
}

#[derive(Serialize, Deserialize)]
pub struct Asked {
    pub id: WaitId,
    pub doc: Option<WaitDoc>,
    #[serde(default)]
    pub open: Vec<WaitId>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct AnsweredRequest {
    #[serde(default)]
    pub ids: Vec<WaitId>,
}

#[derive(Deserialize)]
struct QuestionRequest {
    id: WaitId,
}

#[derive(Serialize)]
struct QuestionState {
    answered: bool,
}

/// 사람이 화면에서 `loom ask` 질문에 쓴 답. `images`는 답에 붙인 그림의 데몬 기계 경로다(`POST /api/pastes`가 준 값, 목업 M30).
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct AnswerWait {
    pub text: String,
    #[serde(default)]
    pub images: Vec<PathBuf>,
}

/// 사람이 알림에서 고른 승인 창 선택지. `prompt`는 화면이 보인 창 그대로, `option`은 그 `options`의 차례다.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct ChooseOption {
    pub prompt: Prompt,
    #[ts(type = "number")]
    pub option: usize,
}

#[derive(Serialize, Deserialize)]
pub struct SaidRequest {
    pub text: String,
}

#[derive(Serialize, Deserialize)]
pub struct HookWaitRequest {
    pub wait: Option<(WaitKind, String)>,
}

#[derive(Serialize, Deserialize)]
pub struct DoneRequest {
    pub summary: String,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub asked: Option<String>,
    #[serde(default)]
    pub images: Vec<PathBuf>,
    #[serde(default)]
    pub doc: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
pub struct Reported {
    pub alive: Vec<String>,
    pub warning: Option<String>,
}

const SCREEN_QUIET: Duration = Duration::from_millis(500);

const NODE_IP: Ipv4Addr = Ipv4Addr::new(127, 76, 77, 1);

pub async fn serve(listen: SocketAddr) -> std::io::Result<()> {
    let claim = launch::Claim::take()?;
    let listener = tokio::net::TcpListener::bind(listen).await?;
    let listen = listener.local_addr()?;
    let node_door = match tokio::net::TcpListener::bind((NODE_IP, listen.port())).await {
        Ok(door) => Some(door),
        Err(_) => tokio::net::TcpListener::bind((NODE_IP, 0)).await.ok(),
    };
    let codex_version = codex::installed_version();
    let db_path = db::default_path()?;
    let node_door_addr = node_door.as_ref().map(|d| d.local_addr()).transpose()?;
    let app = Arc::new(App::new(
        db::open(&db_path)?,
        &db_path,
        listen,
        node_door_addr,
        codex_version.as_deref(),
    )?);
    match app
        .repos
        .add(&std::env::current_dir()?.to_string_lossy())
        .await
    {
        Ok(_) | Err(RepoProblem::NotGit(_) | RepoProblem::NotFound(_)) => {}
        Err(problem) => return Err(std::io::Error::other(problem.to_string())),
    }
    if let Some((repo, path)) = app
        .repos
        .list()
        .map_err(std::io::Error::other)?
        .into_iter()
        .next()
    {
        app.graph
            .settle(&Place {
                repo,
                worktree: path.to_string_lossy().into_owned(),
            })
            .map_err(std::io::Error::other)?;
    }
    app.resume().await.map_err(std::io::Error::other)?;
    if let Err(e) = app.relaunch_stale() {
        eprintln!("loom: 시작 글이 바뀐 노드를 고르지 못했다: {e}");
    }
    app.watch_sleep();
    let node_routes = Router::new()
        .route("/api/agent/note", post(agent_note))
        .route("/api/agent/wait", post(agent_wait))
        .route("/api/agent/answer", post(agent_answer))
        .route("/api/agent/status", post(agent_status))
        .route("/api/agent/nodes", post(agent_nodes))
        .route("/api/agent/take", post(agent_take))
        .route("/api/agent/idle", post(agent_idle))
        .route("/api/agent/ack", post(agent_ack))
        .route("/api/agent/guard", post(agent_guard))
        .route("/api/agent/model", post(agent_model))
        .route("/api/agent/roles", post(agent_roles))
        .route("/api/agent/question", post(agent_question))
        .route("/api/agent/done", post(agent_done))
        .route("/api/agent/ask", post(agent_ask))
        .route("/api/agent/hook-wait", post(agent_hook_wait))
        .route("/api/agent/answered", post(agent_answered))
        .route("/api/agent/said", post(agent_said))
        .route("/api/agent/turn-end", post(agent_turn_end))
        .route("/api/agent/adopt", post(agent_adopt))
        .route("/api/agent/rm", post(agent_remove))
        .route("/api/agent/relaunch", post(relaunch::agent_relaunch))
        .route("/api/agent/ready", post(gate::agent_ready))
        .route("/api/agent/verdict", post(gate::agent_verdict))
        .route("/api/agent/merge", post(gate::agent_merge))
        .route("/api/agent/merged", post(gate::agent_merged))
        .route("/api/agent/fail", post(gate::agent_fail))
        .route("/api/agent/task", post(agent_task));
    let router = node_routes
        .clone()
        .route("/api/repos", get(list_repos).post(add_repo))
        .route("/api/repos/clone", post(clone_repo))
        .route("/api/repos/create", post(create_repo))
        .route("/api/repos/{id}", delete(remove_repo))
        .route("/api/folders", get(list_folders))
        .route(
            "/api/repos/{id}/worktrees",
            post(create_worktree).delete(delete_worktree),
        )
        .route("/api/repos/{id}/worktrees/changes", get(worktree_changes))
        .route("/api/repos/{id}/worktrees/name", put(rename_worktree))
        .route("/api/repos/{id}/commits", get(commits))
        .route("/api/repos/{id}/spots", delete(clear_spots))
        .route("/api/spots", get(list_spots).put(put_spot))
        .route("/api/spots/arranged", put(put_arranged))
        .route(
            "/api/sizes",
            get(list_sizes).put(put_sizes).delete(clear_sizes),
        )
        .route("/api/nodes", post(create_node))
        .route("/api/agent/spawn", post(agent_spawn))
        .route("/api/nodes/{id}", delete(remove_node))
        .route("/api/nodes/{id}/parent/{parent}", put(adopt_node))
        .route("/api/nodes/{id}/name", put(rename_node))
        .route("/api/nodes/{id}/steer-queued", post(steer_queued))
        .route(
            "/api/nodes/{id}/bookmark",
            put(bookmark_node).delete(unbookmark_node),
        )
        .route(
            "/api/nodes/{id}/card-view",
            put(set_card_view).delete(clear_card_view),
        )
        .route("/api/waits/{wait}", delete(dismiss_wait))
        .route("/api/waits/{wait}/answer", post(answer_wait))
        .route("/api/waits/{wait}/choose", post(choose_wait))
        .route("/api/docs", get(list_docs))
        .route(
            "/api/docs/{id}",
            post(open_doc).layer(axum::middleware::from_fn(origin::from_page)),
        )
        .route("/api/docs/{id}/", get(doc))
        .route("/api/docs/{id}/{*file}", get(doc_sibling))
        .route("/api/waits/{wait}/images/{index}", get(done_image_file))
        .route("/api/nodes/{id}/pty", get(pty))
        .route(
            "/api/pastes/{name}",
            post(save_paste).layer(DefaultBodyLimit::max(paste::LIMIT_BYTES)),
        )
        .route(
            "/api/files/open",
            post(open_file).layer(axum::middleware::from_fn(origin::from_page)),
        )
        .route("/api/files/view/{node}/{*path}", get(view_file))
        .route("/api/shutdown", post(shutdown))
        .route("/api/update", get(update_view))
        .route("/api/update/check", post(check_update))
        .route("/api/update/install", post(install_update))
        .route("/api/events", get(events))
        .route("/api/events/log", get(event_log))
        .merge(settings::routes())
        .merge(places::routes())
        .merge(switch::routes())
        .merge(phone::routes())
        .merge(passes::routes())
        .merge(relaunch::routes())
        .merge(carry::routes())
        .fallback(asset)
        .layer(axum::middleware::from_fn_with_state(
            app.devices.clone(),
            origin::human_site,
        ))
        .with_state(app.clone());
    app.door.set_router(router.clone());
    if !app
        .devices
        .list()
        .map_err(std::io::Error::other)?
        .is_empty()
    {
        let app = app.clone();
        tokio::spawn(async move {
            if let Err(e) = app.door.open().await {
                eprintln!("loom: phone door: {e}");
            }
        });
    }
    claim.announce(&node_url(listen))?;
    tokio::spawn(app.update.clone().run());
    println!("loom: http://{listen}");
    let main = axum::serve(listener, router).into_future();
    match node_door {
        Some(door) => {
            let node_routes = node_routes.layer(axum::middleware::from_fn(origin::same_site));
            let node = axum::serve(door, node_routes.with_state(app)).into_future();
            tokio::try_join!(main, node).map(drop)
        }
        None => main.await,
    }
}

fn node_url(listen: SocketAddr) -> String {
    let ip = match listen.ip() {
        ip if ip.is_unspecified() => Ipv4Addr::LOCALHOST.into(),
        ip => ip,
    };
    format!("http://{}", SocketAddr::new(ip, listen.port()))
}

fn node_path() -> std::io::Result<OsString> {
    let exe_dir = std::env::current_exe()?
        .parent()
        .map(PathBuf::from)
        .unwrap_or_default();
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let dirs = std::iter::once(exe_dir).chain(std::env::split_paths(&inherited));
    std::env::join_paths(dirs).map_err(std::io::Error::other)
}

async fn list_repos(State(app): State<Arc<App>>) -> Result<Json<Vec<RepoView>>, Problem> {
    let mut views = Vec::new();
    for (id, path) in app.repos.list().map_err(internal)? {
        views.push(app.repo_view(id, &path).await);
    }
    Ok(Json(views))
}

async fn add_repo(
    State(app): State<Arc<App>>,
    Json(req): Json<NewRepo>,
) -> Result<Json<RepoView>, Problem> {
    let id = app.repos.add(req.path.trim()).await?;
    Ok(Json(app.repo_view(id, &app.repos.path(id)?).await))
}

async fn clone_repo(
    State(app): State<Arc<App>>,
    Json(req): Json<CloneRepo>,
) -> Result<Json<RepoView>, Problem> {
    let id = app.repos.clone(&req.url, req.parent.trim()).await?;
    Ok(Json(app.repo_view(id, &app.repos.path(id)?).await))
}

async fn create_repo(
    State(app): State<Arc<App>>,
    Json(req): Json<CreateRepo>,
) -> Result<Json<RepoView>, Problem> {
    let id = app.repos.create(req.parent.trim(), &req.name).await?;
    Ok(Json(app.repo_view(id, &app.repos.path(id)?).await))
}

async fn remove_repo(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
) -> Result<Json<()>, Problem> {
    app.repos.remove(RepoId(id))?;
    Ok(Json(()))
}

async fn list_folders(Query(req): Query<FolderQuery>) -> Result<Json<FolderList>, Problem> {
    let path = match req.path {
        Some(path) => path,
        None => worktree::loom_home()
            .map_err(internal)?
            .to_string_lossy()
            .into_owned(),
    };
    Ok(Json(worktree::folders(path.trim())?))
}

async fn create_worktree(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
    Json(req): Json<NewWorktree>,
) -> Result<Json<RepoView>, Problem> {
    let id = RepoId(id);
    let repo = app.repos.path(id)?;
    let workspace = app.workspace(&repo).await?;
    let made = worktree::create(&repo, &repo, &workspace, &req.branch, &req.base).await?;
    app.setup(&Place {
        repo: id,
        worktree: made,
    })
    .await?;
    Ok(Json(app.repo_view(id, &repo).await))
}

async fn worktree_changes(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
    Query(req): Query<WorktreePath>,
) -> Result<Json<Vec<String>>, Problem> {
    let repo = app.repos.path(RepoId(id))?;
    match worktree::find(&repo, &req.path).await {
        Ok(listed) => Ok(Json(worktree::changes(&listed).await?)),
        Err(_) => Ok(Json(Vec::new())),
    }
}

async fn rename_worktree(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
    Json(req): Json<RenameWorktree>,
) -> Result<Json<RepoView>, Problem> {
    let id = RepoId(id);
    let repo = app.repos.path(id)?;
    let listed = worktree::find(&repo, &req.path).await?;
    worktree::rename(&repo, &listed, &req.name).await?;
    Ok(Json(app.repo_view(id, &repo).await))
}

async fn delete_worktree(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
    Query(req): Query<DeleteWorktree>,
) -> Result<Json<RepoView>, Problem> {
    let id = RepoId(id);
    let repo = app.repos.path(id)?;
    let listed = worktree::find(&repo, &req.path).await.ok();
    if listed.as_ref().is_some_and(|w| w.original) {
        return Err(RepoProblem::Original.into());
    }
    for node in app.graph.nodes_in(&req.path).map_err(internal)? {
        app.remove(node).map_err(internal)?;
    }
    app.spots.forget_worktree(id, &req.path).map_err(internal)?;
    app.graph.publish_view_changed();
    if let Some(listed) = listed {
        worktree::remove(&repo, &listed).await?;
        if let (true, Some(branch)) = (req.delete_branch, &listed.branch) {
            worktree::delete_branch(&repo, branch).await?;
        }
    }
    Ok(Json(app.repo_view(id, &repo).await))
}

async fn list_spots(State(app): State<Arc<App>>) -> Result<Json<Layout>, Problem> {
    Ok(Json(app.spots.layout().map_err(internal)?))
}

async fn put_spot(
    State(app): State<Arc<App>>,
    Json(placed): Json<Vec<Placed>>,
) -> Result<Json<()>, Problem> {
    app.spots.put(&placed).map_err(internal)?;
    app.graph.publish_view_changed();
    Ok(Json(()))
}

async fn put_arranged(
    State(app): State<Arc<App>>,
    Json(arranged): Json<Arranged>,
) -> Result<Json<()>, Problem> {
    app.spots.put_arranged(&arranged).map_err(internal)?;
    app.graph.publish_view_changed();
    Ok(Json(()))
}

async fn clear_spots(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
) -> Result<Json<()>, Problem> {
    app.spots.clear(RepoId(id)).map_err(internal)?;
    app.graph.publish_view_changed();
    Ok(Json(()))
}

async fn list_sizes(State(app): State<Arc<App>>) -> Result<Json<Vec<CardSize>>, Problem> {
    Ok(Json(app.spots.sizes().map_err(internal)?))
}

async fn put_sizes(
    State(app): State<Arc<App>>,
    Json(sizes): Json<Vec<CardSize>>,
) -> Result<Json<()>, Problem> {
    app.spots.put_sizes(&sizes).map_err(internal)?;
    app.graph.publish_view_changed();
    Ok(Json(()))
}

async fn clear_sizes(
    State(app): State<Arc<App>>,
    Json(nodes): Json<Vec<NodeId>>,
) -> Result<Json<()>, Problem> {
    app.spots.clear_sizes(&nodes).map_err(internal)?;
    app.graph.publish_view_changed();
    Ok(Json(()))
}

async fn commits(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
    Query(range): Query<commit_graph::CommitRange>,
) -> Result<Json<CommitGraph>, Problem> {
    let repo = app.repos.path(RepoId(id))?;
    let read = tokio::try_join!(
        commit_graph::read(&repo, range),
        commit_graph::version(&repo)
    );
    let (commits, version) = match read {
        Ok(read) => read,
        Err(e) => {
            return Err(worktree::check(&repo)
                .await
                .err()
                .map_or_else(|| internal(e), Into::into));
        }
    };
    Ok(Json(CommitGraph {
        commits,
        version,
        worktrees: worktree::worktrees(&repo).await,
    }))
}

impl App {
    fn new(
        database: db::Database,
        db_path: &std::path::Path,
        listen: SocketAddr,
        node_door: Option<SocketAddr>,
        codex_version: Option<&str>,
    ) -> std::io::Result<App> {
        let repos = Repos::new(database.clone());
        let graph = Arc::new(Graph::open(database.clone()).map_err(std::io::Error::other)?);
        let pty_dir = db_path.with_file_name("pty");
        let update = Arc::new(Updater::new(listen, {
            let graph = graph.clone();
            move || graph.publish_view_changed()
        }));
        let devices = Arc::new(Devices::new(database.clone()));
        let idle = idle::IdleWaits::new(graph.clone());
        Ok(App {
            repos,
            host: NodeHost::new(pty_dir.join("nodes")),
            graph,
            settings: Settings::new(database.clone()),
            devices: devices.clone(),
            port: listen.port(),
            door: lan::Door::new(
                db_path
                    .parent()
                    .expect("DB 파일에는 폴더가 있다")
                    .to_path_buf(),
                listen.port(),
                devices,
            ),
            passes: passes::Passes::default(),
            spots: Spots::new(database.clone()),
            database,
            accounts_root: db_path.with_file_name("accounts"),
            logins: settings::Logins::new(pty_dir.join("logins")),
            codex: Mutex::default(),
            opencode2: Mutex::default(),
            sessions: sessions::Sessions::default(),
            first_inputs: sessions::FirstInputs::default(),
            setups: Mutex::default(),
            idle,
            relaunches: relaunch::Relaunches::default(),
            sleeps: sleep::Sleeps::default(),
            switch: switch::Switches::default(),
            usage: usage_cache::UsageCache::default(),
            turn_ends: turn_end::TurnEnds::default(),
            plugin: {
                let plugin =
                    skills::Plugin::install(db_path.parent().expect("DB 파일에는 폴더가 있다"))?;
                for dir in plugin.dirs() {
                    profile::cursor::write_plugin(&dir)?;
                }
                plugin
            },
            shim: {
                let shim = db_path.with_file_name("wsl");
                if cfg!(windows) {
                    wsl::write_shim(&shim, &std::env::current_exe()?)?;
                }
                shim
            },
            homes: wsl::Homes::default(),
            version_warnings: profile::version_warnings(codex_version),
            codex_config: match (codex_version, node_door) {
                (Some(version), Some(_)) => codex::sandbox_args(version, NODE_IP.into()),
                _ => Vec::new(),
            },
            url: match node_door {
                Some(door) => format!("http://{door}"),
                None => node_url(listen),
            },
            spawn_url: node_url(listen),
            path: node_path()?,
            update,
        })
    }
}

impl App {
    fn tree_limits(&self) -> Result<TreeLimits, Problem> {
        Ok(self.settings.get().map_err(internal)?.tree_limits)
    }

    async fn start_root(self: &Arc<Self>, place: &Place, start: &Start) -> Result<NodeId, Problem> {
        let cwd = std::path::Path::new(&place.worktree);
        let made = match start.agent() {
            Some(agent) => {
                let repo = self.repos.path(place.repo)?;
                let name = self.graph.next_root_name(Some(&agent)).map_err(internal)?;
                let tasks = self.tasks_for(&place.worktree).await?;
                Some(tasks.new_dir(&task::base(&repo, &name)).map_err(internal)?)
            }
            None => None,
        };
        let dir = made.as_ref().map(|(dir, _)| dir);
        let prepared = self.prepare(start, cwd, Position::Root, dir).await?;
        let discard = prepared.discarder();
        let id = self
            .graph
            .add_root(
                place,
                start.agent(),
                dir.map(|d| d.folder.as_str()),
                &prepared.launched(),
                |id| {
                    if let Some(dir) = dir {
                        task::create(&dir.local, "")
                            .map_err(|e| format!("{}: {e}", dir.local.display()))?;
                    }
                    self.launch(id, prepared)
                },
            )
            .inspect_err(|_| discard())?;
        if let Some((_, fresh)) = made {
            fresh.keep();
        }
        Ok(id)
    }

    async fn tasks_for(&self, worktree: &str) -> Result<task::Tasks, Problem> {
        match wsl::of(std::path::Path::new(worktree)) {
            Some(at) => {
                let home = self.homes.get(&at.distro).await.map_err(LaunchError::Wsl)?;
                Ok(task::Tasks::wsl(&home, |linux| {
                    PathBuf::from(wsl::windows_path(&at.distro, linux))
                }))
            }
            None => task::Tasks::pc().ok_or_else(|| internal("홈 폴더를 찾지 못했다")),
        }
    }

    async fn task_dir(&self, node: NodeId) -> Result<task::TaskDir, Problem> {
        let place = self.graph.place_of(node).map_err(internal)?;
        let tasks = self.tasks_for(&place.worktree).await?;
        let dir = match self.graph.task_of(node).map_err(internal)? {
            Some(folder) => tasks.dir(&folder),
            None => {
                let repo = self.repos.path(place.repo)?;
                let name = self.graph.relations(node).map_err(internal)?.me.name;
                let (new, fresh) = tasks.new_dir(&task::base(&repo, &name)).map_err(internal)?;
                let kept = self.graph.keep_task(node, &new.folder).map_err(internal)?;
                if kept == new.folder {
                    fresh.keep();
                }
                tasks.dir(&kept)
            }
        };
        task::create(&dir.local, "")
            .map_err(|e| internal(format!("{}: {e}", dir.local.display())))?;
        Ok(dir)
    }

    async fn setup(self: &Arc<Self>, place: &Place) -> Result<(), Problem> {
        let cwd = std::path::Path::new(&place.worktree);
        let Some(line) = worktree::setup_script(cwd) else {
            return Ok(());
        };
        let start = Start::Command { line };
        let prepared = self.prepare(&start, cwd, Position::Root, None).await?;
        let discard = prepared.discarder();
        self.graph
            .add_root(place, start.agent(), None, &prepared.launched(), |id| {
                self.setups.lock().unwrap().insert(id);
                self.launch(id, prepared)
            })
            .inspect_err(|_| discard())?;
        Ok(())
    }

    fn remove(&self, node: NodeId) -> rusqlite::Result<()> {
        self.graph.remove(node)?;
        self.host.kill(node);
        Ok(())
    }

    fn launch(self: &Arc<Self>, id: NodeId, prepared: Prepared) -> Result<(), String> {
        let Prepared {
            cmd,
            setup,
            token,
            profile,
            codex,
            opencode2,
            first_input,
            account: _,
            inside,
            rules: _,
        } = prepared;
        self.first_inputs.launched(id, first_input);
        let distro = inside.map(|inside| inside.distro);
        let on_exit = self.on_exit(id, distro, &token, &codex, &opencode2);
        self.host.spawn(id, cmd, setup, on_exit)?;
        self.follow_screen(id, profile, &token);
        self.follow_servers(id, codex, opencode2, &token);
        Ok(())
    }

    async fn resume(self: &Arc<Self>) -> rusqlite::Result<()> {
        for node in self.graph.active()? {
            let (codex, opencode2) = match node.server.clone() {
                Some(ServerLink::Codex { url, pid, thread }) => {
                    match CodexServer::reconnect(url, pid, thread).await {
                        Ok(server) => (Some(Arc::new(server)), None),
                        Err(e) => {
                            eprintln!(
                                "loom: 노드 {}의 app-server에 다시 붙지 못했다: {e}",
                                node.id.0
                            );
                            (None, None)
                        }
                    }
                }
                Some(ServerLink::OpenCode2 {
                    url,
                    pid,
                    session,
                    password,
                }) => (
                    None,
                    Some(Arc::new(opencode2::Node::reconnect(
                        url, pid, session, password,
                    ))),
                ),
                None => (None, None),
            };
            let profile = node.agent.as_ref().and_then(profile::profile_of);
            let distro = self
                .node_wsl(Some(node.id))
                .ok()
                .flatten()
                .map(|at| at.distro);
            let on_exit = self.on_exit(node.id, distro.clone(), &node.token, &codex, &opencode2);
            if let Err(unattached) = self.host.resume(node.id, on_exit) {
                if let Some(server) = &node.server {
                    stop_server(server);
                }
                self.graph.lost(node.id)?;
                if unattached == node_host::Unattached::Gone
                    && let Some(distro) = distro
                {
                    end_leftovers(
                        &tokio::runtime::Handle::current(),
                        node.id,
                        distro,
                        node.token,
                    );
                }
                continue;
            }
            self.follow_screen(node.id, profile, &node.token);
            self.follow_servers(node.id, codex, opencode2, &node.token);
        }
        self.host.end_others();
        self.logins.end_others();
        Ok(())
    }

    fn on_exit(
        self: &Arc<Self>,
        id: NodeId,
        distro: Option<String>,
        token: &str,
        codex: &Option<Arc<CodexServer>>,
        opencode2: &Option<Arc<opencode2::Node>>,
    ) -> impl FnOnce(Exit) + use<> {
        let app = self.clone();
        let discard = discarder(codex, opencode2);
        let runtime = tokio::runtime::Handle::current();
        let token = token.to_owned();
        move |exit| {
            discard();
            if exit != Exit::Lost
                && let Some(distro) = distro
            {
                end_leftovers(&runtime, id, distro, token);
            }
            if app.relaunches.exited(id) {
                return;
            }
            app.codex.lock().unwrap().remove(&id);
            app.opencode2.lock().unwrap().remove(&id);
            app.sessions.exited(id);
            let setup = app.setups.lock().unwrap().remove(&id);
            if exit == Exit::Command(0)
                && (setup || !app.host.has_output(id))
                && app.graph.discard_finished_shell(id).unwrap_or(false)
            {
                app.host.kill(id);
                return;
            }
            let slept = matches!(exit, Exit::Command(_))
                && app.sleeps.stopping(id)
                && app.graph.slept(id).unwrap_or(false);
            let _ = match exit {
                _ if slept => Ok(()),
                Exit::Command(code) => app.graph.exited(id, code),
                Exit::Setup(code) => app.graph.setup_failed(id, code, &app.host.tail(id)),
                Exit::Lost => app.graph.lost(id),
            };
            app.sleeps.exited(id);
            app.graph.wake_mail();
            app.idle.end(id, IdleEnd::Exit);
        }
    }

    fn follow_screen(self: &Arc<Self>, id: NodeId, profile: Option<Profile>, token: &str) {
        let dialogs = profile::dialogs(profile);
        let grammar = profile::prompt_grammar(profile);
        let Some(attached) = self.host.attach(id) else {
            return;
        };
        if dialogs.is_empty() {
            return;
        }
        let mut started = self.sessions.launched(id, token);
        let (app, token) = (self.clone(), token.to_owned());
        tokio::spawn(async move {
            let mut output = attached.output;
            let mut changed = true;
            loop {
                if !changed {
                    tokio::select! {
                        received = output.recv() => match received {
                            Ok(_) | Err(RecvError::Lagged(_)) => {}
                            Err(RecvError::Closed) => return,
                        },
                        sent = started.changed() => if sent.is_err() {
                            return;
                        },
                    }
                }
                loop {
                    match tokio::time::timeout(SCREEN_QUIET, output.recv()).await {
                        Ok(Ok(_) | Err(RecvError::Lagged(_))) => {}
                        Ok(Err(RecvError::Closed)) => return,
                        Err(_) => break,
                    }
                }
                changed = false;
                let now = app.host.screen(id);
                let found = screen::find(dialogs, &now.text, *started.borrow_and_update());
                let wait = found.as_ref().map(|(kind, text)| (*kind, text.as_str()));
                if let Err(e) = app.graph.set_wait(&token, WaitSource::Screen, wait) {
                    eprintln!("loom: 노드 {}의 응답 대기를 쓰지 못했다: {e}", id.0);
                }
                let prompt = grammar.and_then(|g| screen::prompt(g, &now.text, now.cols.into()));
                if let Err(e) = app.graph.set_prompt(id, prompt) {
                    eprintln!("loom: 노드 {}의 승인 창을 쓰지 못했다: {e}", id.0);
                }
            }
        });
    }

    fn follow_servers(
        self: &Arc<Self>,
        id: NodeId,
        codex: Option<Arc<CodexServer>>,
        opencode2: Option<Arc<opencode2::Node>>,
        token: &str,
    ) {
        if let Some(server) = codex {
            self.codex.lock().unwrap().insert(id, server.clone());
            let (app, mut limited) = (self.clone(), server.limited());
            tokio::spawn(async move {
                while limited.changed().await.is_ok() {
                    app.rate_limited(id, profile::Provider::Codex);
                }
            });
            tokio::spawn(follow_codex_model(
                self.graph.clone(),
                id,
                token.to_owned(),
                server.clone(),
            ));
            let (app, mut thread, token) = (self.clone(), server.thread(), token.to_owned());
            tokio::spawn(async move {
                if thread.wait_for(Option::is_some).await.is_ok() {
                    app.sessions.started(id, &token);
                }
            });
            tokio::spawn(deliver_to_codex(self.clone(), id, server));
        }
        if let Some(node) = opencode2 {
            self.opencode2.lock().unwrap().insert(id, node.clone());
            tokio::spawn(follow_model(self.graph.clone(), id, node.model()));
            let mailbox = NodeMailbox {
                app: self.clone(),
                node: id,
            };
            tokio::spawn(async move { node.run(mailbox).await });
        }
    }

    fn caller(&self, headers: &HeaderMap) -> Result<NodeId, AgentError> {
        let token = bearer(headers)?;
        let node = self
            .graph
            .caller(token)
            .map_err(|e| for_agent(Refusal::from(e)))?
            .ok_or_else(unauthorized)?;
        self.sessions.started(node, token);
        Ok(node)
    }

    async fn prepare(
        &self,
        start: &Start,
        cwd: &std::path::Path,
        position: Position,
        task: Option<&task::TaskDir>,
    ) -> Result<Prepared, Problem> {
        let profile = start.profile();
        let provider = profile.and_then(Profile::provider);
        let settings = self.settings.get().map_err(internal)?;
        let launch = Launch {
            position,
            skills: settings.always_on,
        };
        let token = new_token().map_err(internal)?;
        let mut env = self.node_env(&token).to_vec();
        let plugin = self.plugin.dir(launch.skills);
        let site = match wsl::of(cwd) {
            Some(at) => Site::wsl(
                wsl::inside(&at, &profile::commands(start), &self.shim, &[&plugin])
                    .await
                    .map_err(LaunchError::Wsl)?,
            ),
            None => self.pc_site(),
        };
        let account = provider.and_then(|p| settings.account(p, site.distro()));
        if let Some(dir) = task {
            env.push((task::ENV, dir.shown.clone().into()));
        }
        if let Some(provider) = provider {
            env.extend(site.env(provider, account));
        }
        let inside = site.inside();
        let (plugin, skills) = match inside.as_ref().and_then(|i| i.path(0)) {
            Some(inside) => (
                PathBuf::from(inside),
                PathBuf::from(format!("{inside}/skills")),
            ),
            None => (plugin, self.plugin.skills(launch.skills)),
        };
        let codex = match profile {
            Some(Profile::Codex) => {
                let config: Vec<String> = self
                    .codex_config
                    .iter()
                    .cloned()
                    .chain(codex::server_args(launch))
                    .collect();
                let server = CodexServer::start(cwd, inside, &env, &config)
                    .await
                    .map_err(|cause| LaunchError::Server {
                        program: "codex app-server".into(),
                        cause,
                    })?;
                let call = |method, params| server.call(method, params);
                if let Err(e) = codex::add_skills(call, &skills).await {
                    eprintln!("loom: Codex 서버에 내장 스킬을 더하지 못했다: {e}");
                }
                Some(Arc::new(server))
            }
            _ => None,
        };
        let opencode2 = match profile {
            Some(Profile::OpenCode2) => {
                let password = new_token().map_err(internal)?;
                let resume = match start {
                    Start::Agent { resume, .. } => resume.as_deref(),
                    Start::Command { .. } => None,
                };
                let node =
                    opencode2::Node::start(cwd, inside, &env, &skills, password, launch, resume)
                        .await
                        .map_err(|cause| LaunchError::Server {
                            program: format!("{} serve", opencode2::CMD),
                            cause,
                        })?;
                Some(Arc::new(node))
            }
            _ => None,
        };
        let remote = match (&codex, &opencode2) {
            (Some(server), _) => Some(Remote::Codex { url: server.url() }),
            (_, Some(node)) => Some(node.remote()),
            _ => None,
        };
        let discard = discarder(&codex, &opencode2);
        let mut cmd = profile::command(
            start,
            cwd,
            inside,
            remote,
            &settings.bypass_off,
            &plugin,
            launch,
        )
        .inspect_err(|_| discard())?;
        for (key, value) in &env {
            cmd.env(key, value);
        }
        let inside = site.into_inside();
        if let Some(inside) = &inside {
            cmd = inside.wrap(&cmd);
        }
        Ok(Prepared {
            cmd,
            inside,
            setup: None,
            token,
            account: account.cloned(),
            rules: profile.map(|p| profile::start_digests(p, launch)),
            profile,
            codex,
            opencode2,
            first_input: match start {
                Start::Agent { brief, .. } => !brief.trim().is_empty(),
                Start::Command { .. } => false,
            },
        })
    }

    fn node_wsl(&self, node: Option<NodeId>) -> Result<Option<wsl::WslPath>, Problem> {
        let Some(node) = node else { return Ok(None) };
        let place = self.graph.place_of(node).map_err(internal)?;
        Ok(wsl::of(std::path::Path::new(&place.worktree)))
    }

    fn node_env(&self, token: &str) -> [(&'static str, OsString); 4] {
        [
            ("LOOM_URL", self.url.clone().into()),
            ("LOOM_SPAWN_URL", self.spawn_url.clone().into()),
            ("LOOM_TOKEN", token.into()),
            ("PATH", self.path.clone()),
        ]
    }
}

struct Prepared {
    cmd: portable_pty::CommandBuilder,
    inside: Option<wsl::Inside>,
    setup: Option<portable_pty::CommandBuilder>,
    token: String,
    account: Option<AccountId>,
    rules: Option<profile::StartDigests>,
    profile: Option<Profile>,
    codex: Option<Arc<CodexServer>>,
    opencode2: Option<Arc<opencode2::Node>>,
    first_input: bool,
}

impl Prepared {
    fn launched(&self) -> Launched {
        Launched {
            token: self.token.clone(),
            server: match (&self.codex, &self.opencode2) {
                (Some(server), _) => Some(server.link()),
                (_, Some(node)) => Some(node.link()),
                _ => None,
            },
            account: self.account.clone(),
            rules: self.rules.clone(),
        }
    }

    fn shell(&self, line: &str) -> portable_pty::CommandBuilder {
        match &self.inside {
            Some(inside) => inside.wrap(&portable_pty::CommandBuilder::from_argv(wsl::shell(line))),
            None => profile::shell(line),
        }
    }

    fn discarder(&self) -> impl Fn() + use<> {
        discarder(&self.codex, &self.opencode2)
    }
}

fn end_leftovers(runtime: &tokio::runtime::Handle, id: NodeId, distro: String, token: String) {
    runtime.spawn(async move {
        if let Err(e) = wsl::end_token(&distro, &token).await {
            eprintln!("loom: 노드 {}가 남긴 프로세스를 끝내지 못했다: {e}", id.0);
        }
    });
}

fn discarder(
    codex: &Option<Arc<CodexServer>>,
    opencode2: &Option<Arc<opencode2::Node>>,
) -> impl Fn() + use<> {
    let (codex, opencode2) = (codex.clone(), opencode2.clone());
    move || {
        if let Some(server) = &codex {
            server.stop();
        }
        if let Some(node) = &opencode2 {
            node.stop();
        }
    }
}

struct NodeMailbox {
    app: Arc<App>,
    node: NodeId,
}

impl Mailbox for NodeMailbox {
    fn changed(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        let mut changed = Box::pin(self.app.graph.mail_changed());
        changed.as_mut().enable();
        Box::pin(async move {
            let _ = tokio::time::timeout(REDELIVER_AFTER, changed).await;
        })
    }

    fn take(&self, kinds: &[Kind], limit_bytes: usize) -> Result<Vec<Delivery>, String> {
        self.app
            .take_mail(self.node, kinds, limit_bytes)
            .map_err(|e| e.to_string())
    }

    fn ack(&self, taken: &[Delivery]) -> Result<(), String> {
        let ids: Vec<MessageId> = taken.iter().map(|d| d.id).collect();
        self.app
            .graph
            .ack(self.node, &ids)
            .map_err(|e| e.to_string())
    }

    fn turn_ended(&self, reply: Option<String>) -> Option<String> {
        if let Some(text) = reply {
            self.app.turn_ends.said(self.node, text);
        }
        self.app.turn_end(self.node).map(str::to_owned)
    }

    fn human_input(&self) -> Option<String> {
        self.app.human_input(self.node, None).ok().flatten()
    }
}

async fn follow_model(graph: Arc<Graph>, node: NodeId, mut model: watch::Receiver<Model>) {
    while model.changed().await.is_ok() {
        let now = model.borrow_and_update().clone();
        if let Err(e) = graph.set_model(node, &now) {
            eprintln!("loom: 노드 {}의 모델을 쓰지 못했다: {e}", node.0);
        }
    }
}

fn bearer(headers: &HeaderMap) -> Result<&str, AgentError> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(unauthorized)
}

fn unauthorized() -> AgentError {
    (
        StatusCode::UNAUTHORIZED,
        "Run this only inside a loom node.".into(),
    )
}

async fn follow_codex_model(
    graph: Arc<Graph>,
    node: NodeId,
    token: String,
    server: Arc<CodexServer>,
) {
    let mut thread = server.thread();
    let mut kept = thread.borrow().as_ref().map(|t| t.id.clone());
    while thread.changed().await.is_ok() {
        let Some((id, waiting)) = thread
            .borrow_and_update()
            .as_ref()
            .map(|t| (t.id.clone(), t.waiting))
        else {
            continue;
        };
        if let Err(e) = graph.set_wait(&token, WaitSource::Codex, waiting.map(|kind| (kind, ""))) {
            eprintln!("loom: 노드 {}의 응답 대기를 쓰지 못했다: {e}", node.0);
        }
        if kept.as_ref() != Some(&id) {
            if let Err(e) = graph.set_server(node, &server.link()) {
                eprintln!("loom: 노드 {}의 스레드를 쓰지 못했다: {e}", node.0);
            }
            kept = Some(id.clone());
        }
        let call = |method, params| server.call(method, params);
        let saved = match codex::model(call, &id).await {
            Ok(model) => graph.set_model(node, &model).map_err(|e| e.to_string()),
            Err(e) => Err(e),
        };
        if let Err(e) = saved {
            eprintln!("loom: 노드 {}의 모델을 읽지 못했다: {e}", node.0);
        }
    }
}

async fn deliver_to_codex(app: Arc<App>, node: NodeId, server: Arc<CodexServer>) {
    let mut thread = server.thread();
    let mut started = false;
    let mut busy = false;
    let mut ours = false;
    loop {
        let mut mail = pin!(app.graph.mail_changed());
        mail.as_mut().enable();
        let now = thread.borrow_and_update().clone();
        if let Some(now) = now {
            let was_busy = std::mem::replace(&mut busy, now.status == Status::Busy);
            if !was_busy && busy && !std::mem::take(&mut ours) {
                codex_human_input(&app, node, &server, &now.id).await;
            }
            if was_busy && !busy && !started && codex_turn_end(&app, node, &server, &now.id).await {
                started = true;
                ours = true;
                continue;
            }
            let status = if started { Status::Busy } else { now.status };
            match app.take_mail(node, codex::kinds(status), codex::INPUT_LIMIT_BYTES) {
                Ok(taken) if !taken.is_empty() => {
                    if taken.iter().any(|d| d.kind == Kind::Interrupt) {
                        server.stop_shells();
                    }
                    let call = |method, params| server.call(method, params);
                    match codex::deliver(call, &now.id, status, &taken).await {
                        Ok(()) => {
                            started = true;
                            ours |= status == Status::Idle;
                            let ids: Vec<MessageId> = taken.iter().map(|d| d.id).collect();
                            if let Err(e) = app.graph.ack(node, &ids) {
                                eprintln!("loom: 노드 {}의 받음 확인을 쓰지 못했다: {e}", node.0);
                            }
                        }
                        Err(e) => eprintln!("loom: 노드 {}에 메시지를 넘기지 못했다: {e}", node.0),
                    }
                    continue;
                }
                Ok(_) => {}
                Err(e) => eprintln!("loom: 노드 {}의 메일함을 읽지 못했다: {e}", node.0),
            }
        }
        tokio::select! {
            _ = mail => {}
            _ = tokio::time::sleep(REDELIVER_AFTER) => {}
            changed = thread.changed() => {
                if changed.is_err() {
                    return;
                }
                started = false;
            }
        }
    }
}

impl App {
    fn human_input(&self, node: NodeId, prompt: Option<&str>) -> Result<Option<String>, Refusal> {
        self.graph.human_input(node)?;
        self.turn_ends.human(node);
        if let Some(text) = prompt
            && !self.first_inputs.injected(node)
        {
            self.graph.human_typed(node, text)?;
        }
        Ok(profile::questions_note(&self.graph.open_questions(node)?))
    }
}

async fn codex_human_input(app: &App, node: NodeId, server: &CodexServer, thread: &str) {
    let text = match app.human_input(node, None) {
        Ok(Some(text)) => text,
        Ok(None) => return,
        Err(e) => return eprintln!("loom: 노드 {}의 사람 입력을 처리하지 못했다: {e}", node.0),
    };
    let call = |method, params| server.call(method, params);
    if let Err(e) = codex::steer(call, thread, &text).await {
        eprintln!("loom: 노드 {}에 열린 질문을 알리지 못했다: {e}", node.0);
    }
}

async fn codex_turn_end(app: &App, node: NodeId, server: &CodexServer, thread: &str) -> bool {
    let call = |method, params| server.call(method, params);
    match codex::last_reply(call, thread).await {
        Ok(Some(text)) => app.turn_ends.said(node, text),
        Ok(None) => {}
        Err(e) => eprintln!("loom: 노드 {}의 답 글을 읽지 못했다: {e}", node.0),
    }
    let Some(nudge) = app.turn_end(node) else {
        return false;
    };
    match codex::start_turn(call, thread, nudge).await {
        Ok(()) => true,
        Err(e) => {
            eprintln!("loom: 노드 {}를 다시 일하게 하지 못했다: {e}", node.0);
            false
        }
    }
}

fn stop_server(server: &ServerLink) {
    let (ServerLink::Codex { pid, .. } | ServerLink::OpenCode2 { pid, .. }) = server;
    if let Some(root) = pid {
        process_tree::Snapshot::now().kill_trees([*root]);
    }
}

fn new_token() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn internal(e: impl std::fmt::Display) -> Problem {
    Problem::Internal(e.to_string())
}

async fn create_node(
    State(app): State<Arc<App>>,
    Json(req): Json<NewNode>,
) -> Result<Json<NodeId>, Problem> {
    let repo = app.repos.path(req.place.repo)?;
    worktree::find(&repo, &req.place.worktree).await?;
    Ok(Json(app.start_root(&req.place, &req.start).await?))
}

async fn save_paste(
    State(app): State<Arc<App>>,
    Path(name): Path<String>,
    Query(at): Query<PasteAt>,
    body: Bytes,
) -> Result<Json<String>, Problem> {
    let wsl = app.node_wsl(at.node)?;
    let root = match &wsl {
        Some(wsl) => PathBuf::from(wsl.unc("/tmp")),
        None => std::env::temp_dir(),
    };
    let path = paste::save(&root, &name, &body).map_err(internal)?;
    let path = path.to_string_lossy().into_owned();
    Ok(Json(match wsl::location(&path) {
        Some(inside) if wsl.is_some() => inside.path,
        _ => path,
    }))
}

async fn remove_node(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
) -> Result<Json<()>, Problem> {
    app.remove(NodeId(id)).map_err(internal)?;
    Ok(Json(()))
}

async fn adopt_node(
    State(app): State<Arc<App>>,
    Path((id, parent)): Path<(u32, u32)>,
) -> Result<Json<()>, Problem> {
    let limits = app.tree_limits()?;
    app.graph.adopt(NodeId(parent), NodeId(id), limits)?;
    Ok(Json(()))
}

async fn open_file(
    State(app): State<Arc<App>>,
    Json(req): Json<OpenFile>,
) -> Result<Json<()>, Problem> {
    let path = host_path(&app, req.node, req.path)?;
    open::open(std::path::Path::new(&path)).map_err(internal)?;
    Ok(Json(()))
}

fn host_path(app: &App, node: Option<NodeId>, path: String) -> Result<String, Problem> {
    Ok(match app.node_wsl(node)? {
        Some(wsl) if path.starts_with('/') && !path.starts_with("//") => wsl.shell_path(&path),
        _ => path,
    })
}

async fn view_file(
    State(app): State<Arc<App>>,
    Path((node, path)): Path<(String, String)>,
    request: axum::extract::Request,
) -> Result<Response, Problem> {
    let node = match node.as_str() {
        "-" => None,
        id => match id.parse() {
            Ok(id) => Some(NodeId(id)),
            Err(_) => return Ok(StatusCode::NOT_FOUND.into_response()),
        },
    };
    let path = if path.as_bytes().get(1) == Some(&b':') {
        path
    } else {
        format!("/{path}")
    };
    let path = host_path(&app, node, path)?;
    serve_file(std::path::Path::new(&path), request).await
}

const SHUTDOWN_AFTER: Duration = Duration::from_millis(200);

async fn shutdown() -> StatusCode {
    tokio::spawn(async {
        tokio::time::sleep(SHUTDOWN_AFTER).await;
        std::process::exit(0);
    });
    StatusCode::NO_CONTENT
}

async fn update_view(State(app): State<Arc<App>>) -> Json<UpdateView> {
    Json(app.update.view())
}

async fn check_update(State(app): State<Arc<App>>) -> Result<Json<UpdateView>, Problem> {
    app.update.clone().check().await.map(Json).map_err(internal)
}

async fn install_update(State(app): State<Arc<App>>) -> Result<Json<UpdateView>, Problem> {
    app.update.clone().install().map(Json).map_err(internal)
}

async fn rename_node(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
    Json(req): Json<RenameNode>,
) -> Result<Json<()>, Problem> {
    app.graph.rename(NodeId(id), &req.name)?;
    Ok(Json(()))
}

async fn steer_queued(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
) -> Result<Json<()>, Problem> {
    app.graph.steer_queued(NodeId(id))?;
    Ok(Json(()))
}

async fn bookmark_node(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
) -> Result<Json<()>, Problem> {
    app.graph.bookmark(NodeId(id))?;
    Ok(Json(()))
}

async fn unbookmark_node(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
) -> Result<Json<()>, Problem> {
    app.graph.unbookmark(NodeId(id))?;
    Ok(Json(()))
}

async fn set_card_view(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
    Json(view): Json<CardView>,
) -> Result<Json<()>, Problem> {
    app.graph.set_card_view(NodeId(id), Some(view))?;
    Ok(Json(()))
}

async fn clear_card_view(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
) -> Result<Json<()>, Problem> {
    app.graph.set_card_view(NodeId(id), None)?;
    Ok(Json(()))
}

async fn dismiss_wait(
    State(app): State<Arc<App>>,
    Path(wait): Path<u32>,
) -> Result<Json<()>, Problem> {
    app.graph.dismiss(WaitId(wait))?;
    Ok(Json(()))
}

async fn answer_wait(
    State(app): State<Arc<App>>,
    Path(wait): Path<u32>,
    Json(req): Json<AnswerWait>,
) -> Result<Json<()>, Problem> {
    let node = app.graph.answer(WaitId(wait), &req.text, &req.images)?;
    app.turn_ends.human(node);
    Ok(Json(()))
}

async fn choose_wait(
    State(app): State<Arc<App>>,
    Path(wait): Path<u32>,
    Json(req): Json<ChooseOption>,
) -> Result<Json<()>, Problem> {
    choose(&app.graph, &app.host, WaitId(wait), &req)?;
    Ok(Json(()))
}

fn choose(
    graph: &Graph,
    host: &NodeHost<NodeId>,
    wait: WaitId,
    req: &ChooseOption,
) -> Result<(), Refusal> {
    let (node, agent) = graph.approval(wait)?;
    let grammar = agent
        .as_ref()
        .and_then(profile::profile_of)
        .and_then(|p| profile::prompt_grammar(Some(p)));
    host.write_if(node, |keys| {
        let shown = keys.screen();
        let now = grammar.and_then(|g| screen::prompt(g, &shown.text, shown.cols.into()));
        let option = match now {
            Some(now) if now == req.prompt => now.options.get(req.option).cloned(),
            _ => None,
        };
        let option = option.ok_or(Refusal::PromptChanged(wait))?;
        graph.dismiss(wait)?;
        Ok(option.key.bytes())
    })
}

async fn list_docs() -> Result<Json<Vec<user_doc::DocRow>>, Problem> {
    Ok(Json(user_doc::docs().map_err(internal)?))
}

async fn open_doc(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> Result<Json<()>, DocProblem> {
    doc_file(&id)?;
    open::open_url(&format!("http://127.0.0.1:{}/api/docs/{id}/", app.port)).map_err(internal)?;
    Ok(Json(()))
}

async fn doc(
    Path(id): Path<String>,
    request: axum::extract::Request,
) -> Result<Response, DocProblem> {
    let doc = doc_file(&id)?;
    Ok(serve_file(&doc.path, request).await?)
}

async fn doc_sibling(
    Path((id, file)): Path<(String, String)>,
    request: axum::extract::Request,
) -> Result<Response, DocProblem> {
    let doc = doc_file(&id)?;
    match doc.folder.and_then(|dir| passes::inside(&dir, &file)) {
        Some(path) => Ok(serve_file(&path, request).await?),
        None => Ok(StatusCode::NOT_FOUND.into_response()),
    }
}

pub enum DocProblem {
    Missing(String),
    Other(Problem),
}

impl From<Problem> for DocProblem {
    fn from(e: Problem) -> Self {
        DocProblem::Other(e)
    }
}

impl IntoResponse for DocProblem {
    fn into_response(self) -> Response {
        match self {
            DocProblem::Missing(reason) => (
                StatusCode::NOT_FOUND,
                [(axum::http::header::CONTENT_TYPE, TEXT_TYPE)],
                reason,
            )
                .into_response(),
            DocProblem::Other(problem) => problem.into_response(),
        }
    }
}

fn doc_file(id: &str) -> Result<user_doc::DocFile, DocProblem> {
    user_doc::doc_file(id).map_err(DocProblem::Missing)
}

async fn done_image_file(
    State(app): State<Arc<App>>,
    Path((wait, index)): Path<(u32, usize)>,
    request: axum::extract::Request,
) -> Result<Response, Problem> {
    let path = app.graph.done_image(WaitId(wait), index)?;
    serve_file(&path, request).await
}

const TEXT_TYPE: &str = "text/plain; charset=utf-8";

async fn serve_file(
    path: &std::path::Path,
    request: axum::extract::Request,
) -> Result<Response, Problem> {
    Ok(tower_http::services::ServeFile::new(path)
        .try_call(request)
        .await
        .map_err(internal)?
        .into_response())
}

async fn agent_task(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<TaskRequest>,
) -> Result<Json<Option<String>>, AgentError> {
    let caller = app.caller(&headers)?;
    let dir = match &req.branch {
        None => Some(app.task_dir(caller).await.map_err(for_agent)?),
        Some(branch) => match app.graph.branch_task(caller, branch).map_err(for_agent)? {
            Some((worktree, folder)) => {
                let tasks = app.tasks_for(&worktree).await.map_err(for_agent)?;
                Some(tasks.dir(&folder))
            }
            None => None,
        },
    };
    Ok(Json(
        dir.map(|dir| dir.local.to_string_lossy().into_owned()),
    ))
}

async fn agent_status(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
) -> Result<Json<Relations>, AgentError> {
    let node = app.caller(&headers)?;
    app.graph.relations(node).map(Json).map_err(for_agent)
}

async fn agent_nodes(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
) -> Result<Json<Vec<Listed>>, AgentError> {
    app.caller(&headers)?;
    app.graph.directory().map(Json).map_err(for_agent)
}

async fn agent_roles(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
) -> Result<Json<Roles>, AgentError> {
    app.caller(&headers)?;
    let settings = app.settings.get().map_err(|e| for_agent(internal(e)))?;
    Ok(Json(settings.roles))
}

async fn agent_question(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<QuestionRequest>,
) -> Result<Json<QuestionState>, AgentError> {
    app.caller(&headers)?;
    let answered = app
        .graph
        .answered_in_inbox(req.id)
        .map_err(|e| for_agent(internal(e)))?;
    Ok(Json(QuestionState { answered }))
}

async fn agent_take(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<TakeRequest>,
) -> Result<Json<Taken>, AgentError> {
    let node = app.caller(&headers)?;
    app.idle.turn_started(node, req.hook);
    keep_transcript(&app, node, req.transcript.as_deref())?;
    let messages = app
        .graph
        .take(node, &req.kinds, req.limit_bytes)
        .map_err(for_agent)?;
    let note = match req.human_input {
        true => app
            .human_input(node, req.prompt.as_deref())
            .map_err(for_agent)?,
        false => None,
    };
    Ok(Json(Taken { messages, note }))
}

async fn agent_idle(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<Option<IdleRequest>>,
) -> Result<Json<Vec<Delivery>>, AgentError> {
    let node = app.caller(&headers)?;
    let req = req.unwrap_or_default();
    keep_transcript(&app, node, req.transcript.as_deref())?;
    let mut wait = app.idle.open(node);
    if !app.graph.relations(node).map_err(for_agent)?.me.alive {
        app.idle.end(node, IdleEnd::Exit);
        wait.finish(IdleEnd::Exit);
        eprintln!(
            "loom: 노드 {}의 쉬는 대기를 거절했다: 프로세스가 끝났다",
            node.0
        );
        return Err((StatusCode::GONE, "The node process has exited.".to_owned()));
    }
    if req.rate_limited {
        app.rate_limited(node, profile::Provider::Claude);
    }
    app.turn_ended(node);
    while app.relaunches.holds_mail(node) {
        let by = wait.ended().await;
        if app.caller(&headers).is_err() || !app.idle.reopen(&mut wait) {
            wait.finish(by);
            return Ok(Json(Vec::new()));
        }
    }
    tokio::select! {
        taken = app.next_mail(node, Kind::ALL, claude::HOOK_LIMIT_BYTES) => match taken {
            Ok(taken) => {
                wait.finish(IdleEnd::Mail);
                Ok(Json(taken))
            }
            Err(e) => {
                wait.finish(IdleEnd::Error);
                eprintln!("loom: 노드 {}의 쉬는 대기가 끝났다: {e}", node.0);
                Err(for_agent(e))
            }
        },
        by = wait.ended() => {
            wait.finish(by);
            Ok(Json(Vec::new()))
        }
    }
}

async fn agent_ack(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<AckRequest>,
) -> Result<Json<()>, AgentError> {
    let node = app.caller(&headers)?;
    app.graph.ack(node, &req.ids).map_err(for_agent)?;
    Ok(Json(()))
}

async fn agent_guard(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<GuardRequest>,
) -> Result<Json<()>, AgentError> {
    let node = app.caller(&headers)?;
    app.host.guard(node, req.caller);
    Ok(Json(()))
}

async fn agent_model(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(model): Json<Model>,
) -> Result<Json<()>, AgentError> {
    let node = app.caller(&headers)?;
    app.graph
        .set_model(node, &model)
        .map_err(|e| for_agent(Refusal::from(e)))?;
    Ok(Json(()))
}

fn keep_transcript(app: &App, node: NodeId, path: Option<&str>) -> Result<(), AgentError> {
    match path {
        Some(path) => app
            .graph
            .set_transcript(node, path)
            .map_err(|e| for_agent(Refusal::from(e))),
        None => Ok(()),
    }
}

async fn agent_wait(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<WaitRequest>,
) -> Result<Json<Delivery>, AgentError> {
    let node = app.caller(&headers)?;
    let wait = app.graph.wait(node, req.reply_to);
    let result = match req.timeout_secs {
        Some(secs) => tokio::time::timeout(std::time::Duration::from_secs(secs), wait)
            .await
            .map_err(|_| {
                (
                    StatusCode::REQUEST_TIMEOUT,
                    "Timed out while waiting for a message.".to_owned(),
                )
            })?,
        None => wait.await,
    };
    Ok(Json(result.map_err(for_agent)?))
}

async fn agent_said(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<SaidRequest>,
) -> Result<Json<()>, AgentError> {
    let node = app.caller(&headers)?;
    app.turn_ends.said(node, req.text);
    Ok(Json(()))
}

async fn agent_turn_end(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
) -> Result<Json<Option<String>>, AgentError> {
    let node = app.caller(&headers)?;
    Ok(Json(app.turn_end(node).map(str::to_owned)))
}

async fn agent_hook_wait(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<HookWaitRequest>,
) -> Result<Json<()>, AgentError> {
    app.caller(&headers)?;
    app.graph
        .set_wait(
            bearer(&headers)?,
            WaitSource::Hook,
            req.wait.as_ref().map(|(kind, text)| (*kind, text.as_str())),
        )
        .map_err(|e| for_agent(Refusal::from(e)))?;
    Ok(Json(()))
}

async fn pty(
    ws: WebSocketUpgrade,
    Path(id): Path<u32>,
    State(app): State<Arc<App>>,
    via: Option<Extension<ViaRemote>>,
) -> Response {
    let typed = |app: &App, id| {
        if let Err(e) = app.graph.typed(id) {
            eprintln!("loom: 노드 {}의 응답 대기를 지우지 못했다: {e}", id.0);
        }
        app.sleeps.knocked(id);
    };
    let id = NodeId(id);
    if app.host.attach(id).is_none() && app.graph.reachable(id).unwrap_or(false) {
        return coming_pty(ws, via, app, id);
    }
    serve_pty(ws, via, app, |app| &app.host, typed, id)
}

const COMING_PTY_CHECK: Duration = Duration::from_secs(1);

fn coming_pty(
    ws: WebSocketUpgrade,
    via: Option<Extension<ViaRemote>>,
    app: Arc<App>,
    id: NodeId,
) -> Response {
    let devices = app.devices.clone();
    ws.on_upgrade(move |mut socket| {
        origin::until_removed(devices, via, async move {
            let mut check = tokio::time::interval(COMING_PTY_CHECK);
            loop {
                tokio::select! {
                    frame = socket.next() => match frame {
                        Some(Ok(Message::Binary(_))) => app.sleeps.knocked(id),
                        Some(Ok(_)) => {}
                        Some(Err(_)) | None => return,
                    },
                    _ = check.tick() => {
                        if app.host.attach(id).is_some() || !app.graph.reachable(id).unwrap_or(false) {
                            let _ = socket.send(Message::Close(None)).await;
                            return;
                        }
                    }
                }
            }
        })
    })
}

/// 화면이 PTY 웹소켓에 보내는 글 프레임. 키 입력은 바이너리 프레임으로 간다(docs/design.md 「데몬 하나」 PTY 열·줄).
#[derive(Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PtyRequest {
    /// 이 크기를 쥔다. PC 화면이 보낸다.
    Resize(Resize),
    /// 이 연결이 보는 동안만 이 크기를 쓴다. 폰이 보낸다. 연결이 닫히거나 `GiveBack`이 오면 빌리기 전 크기로 돌린다.
    Borrow(Resize),
    /// 빌린 크기를 돌려준다. 폰 화면이 가려질 때 보낸다.
    GiveBack,
}

/// PTY 웹소켓의 글 프레임. 터미널 출력은 바이너리 프레임으로 간다.
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PtyNotice {
    /// 이 PTY가 없다. 데몬이 다시 시작할 때 호스트가 사라졌거나, 앞 데몬에서 끝난 노드다.
    Gone,
    /// PTY의 열·줄. 붙을 때 backlog보다 먼저, 그 뒤 바뀔 때마다 보낸다. 화면은 이 크기로 그리고 칸에 맞게 늘이거나 줄인다.
    Size(Resize),
    /// 붙을 때 PTY가 한동안 조용했다(`node_host::Attached::settled`). backlog보다 먼저 보낸다. 화면은 그 backlog 속 색 질의(OSC 10·11·4)에 답하지 않는다.
    /// 근거: 카드를 열 때마다 xterm이 옛 질의에 다시 답해 프로그램에 늦은 입력이 갔다(실측 2026-10-23). 갓 보낸 질의는 프로그램이 아직 기다릴 수 있어 답한다.
    Settled,
}

fn serve_pty<K: Copy + Eq + Hash + std::fmt::Display + Send + Sync + 'static>(
    ws: WebSocketUpgrade,
    via: Option<Extension<ViaRemote>>,
    app: Arc<App>,
    host: fn(&App) -> &NodeHost<K>,
    typed: fn(&App, K),
    id: K,
) -> Response {
    let Some(attached) = host(&app).attach(id) else {
        let devices = app.devices.clone();
        return ws.on_upgrade(move |mut socket| {
            origin::until_removed(devices, via, async move {
                let notice =
                    serde_json::to_string(&PtyNotice::Gone).expect("PtyNotice는 늘 직렬화된다");
                if socket.send(Message::Text(notice.into())).await.is_ok() {
                    while let Some(Ok(_)) = socket.next().await {}
                }
            })
        });
    };
    let devices = app.devices.clone();
    let borrower = node_host::Borrower::fresh();
    ws.on_upgrade(move |socket| {
        origin::until_removed(devices, via, async move {
            let _give_back = GiveBack {
                app: app.clone(),
                host,
                id,
                borrower,
            };
            relay_pty(socket, attached, &app, host, typed, id, borrower).await;
        })
    })
}

struct GiveBack<K: Copy + Eq + Hash + std::fmt::Display + Send + Sync + 'static> {
    app: Arc<App>,
    host: fn(&App) -> &NodeHost<K>,
    id: K,
    borrower: node_host::Borrower,
}

impl<K: Copy + Eq + Hash + std::fmt::Display + Send + Sync + 'static> Drop for GiveBack<K> {
    fn drop(&mut self) {
        (self.host)(&self.app).give_back(self.id, self.borrower);
    }
}

async fn relay_pty<K: Copy + Eq + Hash + std::fmt::Display + Send + Sync + 'static>(
    socket: WebSocket,
    attached: node_host::Attached,
    app: &App,
    host: fn(&App) -> &NodeHost<K>,
    typed: fn(&App, K),
    id: K,
    borrower: node_host::Borrower,
) {
    let (mut to_view, mut from_view) = socket.split();
    let mut output = attached.output;
    let mut size = attached.size;
    if send_size(&mut to_view, &mut size).await.is_err() {
        return;
    }
    if attached.settled {
        let notice = serde_json::to_string(&PtyNotice::Settled).expect("PtyNotice는 늘 직렬화된다");
        if to_view.send(Message::Text(notice.into())).await.is_err() {
            return;
        }
    }
    if to_view
        .send(Message::Binary(attached.backlog))
        .await
        .is_err()
    {
        return;
    }
    loop {
        tokio::select! {
            chunk = output.recv() => match chunk {
                Ok(bytes) => if to_view.send(Message::Binary(bytes)).await.is_err() { return },
                Err(RecvError::Lagged(_) | RecvError::Closed) => return,
            },
            changed = size.changed() => {
                if changed.is_err() || send_size(&mut to_view, &mut size).await.is_err() {
                    return;
                }
            }
            msg = from_view.next() => match msg {
                Some(Ok(Message::Binary(bytes))) => {
                    let _ = host(app).write_if(id, |_| {
                        if node_host::typed(&bytes) {
                            typed(app, id);
                        }
                        Ok::<_, std::convert::Infallible>(bytes.to_vec())
                    });
                }
                Some(Ok(Message::Text(text))) => match serde_json::from_str(&text) {
                    Ok(PtyRequest::Resize(Resize { cols, rows })) => host(app).resize(id, cols, rows),
                    Ok(PtyRequest::Borrow(Resize { cols, rows })) => host(app).borrow(id, borrower, cols, rows),
                    Ok(PtyRequest::GiveBack) => host(app).give_back(id, borrower),
                    Err(_) => {}
                },
                Some(Ok(_)) => {}
                Some(Err(_)) | None => return,
            },
        }
    }
}

async fn send_size(
    to_view: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    size: &mut tokio::sync::watch::Receiver<node_host::PtySize>,
) -> Result<(), axum::Error> {
    let node_host::PtySize { cols, rows } = *size.borrow_and_update();
    let notice = serde_json::to_string(&PtyNotice::Size(Resize { cols, rows }))
        .expect("PtyNotice는 늘 직렬화된다");
    to_view.send(Message::Text(notice.into())).await
}

async fn event_log(State(app): State<Arc<App>>) -> Result<Json<Vec<EventRecord>>, Problem> {
    app.graph.recent_events().map(Json).map_err(internal)
}

async fn events(
    ws: WebSocketUpgrade,
    State(app): State<Arc<App>>,
    via: Option<Extension<ViaRemote>>,
) -> Response {
    let devices = app.devices.clone();
    ws.on_upgrade(move |socket| origin::until_removed(devices, via, send_events(socket, app)))
}

async fn send_events(mut socket: WebSocket, app: Arc<App>) {
    let Ok((snapshot, mut rx)) = app.graph.subscribe() else {
        return;
    };
    if send_json(&mut socket, &GraphEvent::Snapshot { nodes: snapshot })
        .await
        .is_err()
    {
        return;
    }
    while let Ok(event) = rx.recv().await {
        if send_json(&mut socket, &event).await.is_err() {
            return;
        }
    }
}

async fn send_json(socket: &mut WebSocket, value: &impl Serialize) -> Result<(), axum::Error> {
    let text = serde_json::to_string(value).expect("GraphEvent는 늘 직렬화된다");
    socket.send(Message::Text(text.into())).await
}

const HASHED_ASSETS: &str = "assets/";

async fn asset(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let Some(file) = Assets::get(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut response = (
        [(header::CONTENT_TYPE, file.metadata.mimetype().to_owned())],
        file.data,
    )
        .into_response();
    if path.starts_with(HASHED_ASSETS) {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_agent_route_the_cli_calls_is_on_the_node_door() {
        let cli = include_str!("cli.rs");
        let daemon = include_str!("daemon.rs");
        let door = &daemon[daemon.find("let node_routes = Router::new()").unwrap()..];
        let door = &door[..door.find("let router = node_routes").unwrap()];
        let mut names = std::collections::BTreeSet::new();
        for part in cli.split("call(").skip(1) {
            let part = part.trim_start();
            if let Some(rest) = part.strip_prefix('"')
                && let Some((name, _)) = rest.split_once('"')
                && name.chars().all(|c| c.is_ascii_lowercase() || c == '-')
            {
                names.insert(name);
            }
        }
        assert!(
            names.contains("task") && names.contains("verdict"),
            "{names:?}"
        );
        for name in names {
            assert!(
                door.contains(&format!("\"/api/agent/{name}\"")),
                "the CLI calls {name}, but the node door has no /api/agent/{name}"
            );
        }
    }

    #[tokio::test]
    async fn only_hashed_assets_are_cached() {
        let cache = |path: String| async move {
            let response = asset(path.parse().unwrap()).await;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            response.headers().get(header::CACHE_CONTROL).cloned()
        };
        let hashed = Assets::iter()
            .find(|p| p.starts_with(HASHED_ASSETS))
            .expect("web/dist has assets/");
        assert_eq!(
            cache(format!("/{hashed}")).await.unwrap(),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(cache("/manifest.webmanifest".into()).await, None);
    }

    #[cfg(unix)]
    #[test]
    fn a_choice_types_once_and_stale_choices_type_nothing() {
        const SCREEN: &str = "──────\n Bash command\n╌╌╌╌\n touch one\n╌╌╌╌\n Do you want to proceed?\n ❯ 1. Yes\n   2. No\n\n";
        let dir = tempfile::tempdir().unwrap();
        let db = db::open(&dir.path().join("loom.db")).unwrap();
        db.lock()
            .execute("INSERT INTO repos (id, path) VALUES (1, '/repo')", [])
            .unwrap();
        let graph = Graph::open(db).unwrap();
        let place = Place {
            repo: RepoId(1),
            worktree: "/repo".into(),
        };
        let launched = Launched {
            token: "t".into(),
            server: None,
            account: None,
            rules: None,
        };
        let node = graph
            .add_root(
                &place,
                Some(AgentId("claude".into())),
                None,
                &launched,
                |_| Ok(()),
            )
            .unwrap();
        let screen_cmd = || {
            let mut cmd = portable_pty::CommandBuilder::new("sh");
            cmd.args([
                "-c",
                &format!("stty -icanon -echo; printf '{SCREEN}'; exec cat"),
            ]);
            cmd
        };
        let hosts = dir.path().join("hosts");
        node_host::in_process(&hosts, node, screen_cmd());
        let host = NodeHost::new(hosts.clone());
        let (exited, exit) = std::sync::mpsc::channel();
        host.resume(node, move |_| exited.send(()).unwrap())
            .unwrap();
        let shown = loop {
            let text = host.screen(node).text;
            if let Some(prompt) = screen::prompt(&claude::PROMPT, &text, 80) {
                break prompt;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let mut typed = host.attach(node).unwrap().output;
        let open = |token: &str| {
            graph
                .set_wait(
                    token,
                    WaitSource::Hook,
                    Some((WaitKind::Approval, "permission")),
                )
                .unwrap();
            let views = graph.subscribe().unwrap().0;
            let view = views.into_iter().find(|v| v.id == node).unwrap();
            view.waits.iter().map(|w| w.id).collect::<Vec<_>>()
        };
        let pick = |wait, prompt: &Prompt| {
            choose(
                &graph,
                &host,
                wait,
                &ChooseOption {
                    prompt: prompt.clone(),
                    option: 0,
                },
            )
        };
        let typed_until_mark = |typed: &mut tokio::sync::broadcast::Receiver<Bytes>| {
            host.write_if(node, |_| Ok::<_, std::convert::Infallible>(b"Z".to_vec()))
                .unwrap();
            let mut seen = String::new();
            while !seen.ends_with('Z') {
                seen.push_str(&String::from_utf8_lossy(&typed.blocking_recv().unwrap()));
            }
            seen
        };

        let wait = open("t")[0];
        let barrier = std::sync::Barrier::new(2);
        let results: Vec<_> = std::thread::scope(|scope| {
            let both: Vec<_> = (0..2)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        pick(wait, &shown)
                    })
                })
                .collect();
            both.into_iter().map(|t| t.join().unwrap()).collect()
        });
        assert!(results.contains(&Ok(())), "{results:?}");
        assert!(results.contains(&Err(Refusal::NoWait(wait))), "{results:?}");

        let wait = open("t")[0];
        graph.typed(node).unwrap();
        assert_eq!(pick(wait, &shown), Err(Refusal::NoWait(wait)));

        let wait = open("t")[0];
        let mut older = shown.clone();
        older.rows[3] = " touch two".into();
        assert_eq!(pick(wait, &older), Err(Refusal::PromptChanged(wait)));
        assert_eq!(graph.approval(wait).unwrap().0, node);

        assert_eq!(typed_until_mark(&mut typed), "1Z");

        host.kill(node);
        exit.recv().unwrap();
        let relaunched = Launched {
            token: "t2".into(),
            server: None,
            account: None,
            rules: None,
        };
        graph.relaunched(node, &relaunched).unwrap();
        node_host::in_process(&hosts, node, screen_cmd());
        host.resume(node, |_| {}).unwrap();
        while screen::prompt(&claude::PROMPT, &host.screen(node).text, 80).as_ref() != Some(&shown)
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut typed = host.attach(node).unwrap().output;
        assert_eq!(pick(wait, &shown), Err(Refusal::NoWait(wait)));
        assert_eq!(open("t"), vec![]);
        let newer = open("t2")[0];
        assert_eq!(pick(wait, &shown), Err(Refusal::NoWait(wait)));
        assert_eq!(graph.approval(newer).unwrap().0, node);
        assert_eq!(typed_until_mark(&mut typed), "Z");
        host.kill(node);
    }

    #[test]
    fn problem_is_a_code_and_value() {
        let json = |p: Problem| serde_json::to_value(p).unwrap();
        assert_eq!(
            json(Refusal::NameTaken("a".into()).into()),
            serde_json::json!({"code": "refused", "value": {"code": "nameTaken", "value": "a"}})
        );
        assert_eq!(
            json(
                LaunchError::BriefTooLong {
                    len: 5000,
                    limit: 4096
                }
                .into()
            ),
            serde_json::json!({"code": "launch", "value": {"code": "briefTooLong", "value": {"len": 5000, "limit": 4096}}})
        );
        assert_eq!(
            json(Problem::NoLogin),
            serde_json::json!({"code": "noLogin"})
        );
        assert_eq!(Problem::NoLogin.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            for_agent(Refusal::EmptyName),
            (StatusCode::BAD_REQUEST, "Give a name.".into())
        );
    }
}
