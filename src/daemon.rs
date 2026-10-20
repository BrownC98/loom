//! 조합 레이어: HTTP·WebSocket 라우트. 아래 기능 모듈을 부르고 결과를 엮기만 한다.
//! 설정·계정·사용량 라우트는 `settings.rs`에 둔다.

mod settings;

use std::collections::HashMap;
use std::ffi::OsString;
use std::future::IntoFuture;
use std::hash::Hash;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::pin::pin;
use std::sync::{Arc, Mutex};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use rust_embed::Embed;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;
use ts_rs::TS;

use crate::accounts::Accounts;
use crate::codex_server::CodexServer;
use crate::commit_graph::{self, Commit};
use crate::db;
use crate::graph::{
    Delivery, EventRecord, Graph, GraphEvent, Kind, Launched, MessageId, Model, NodeId, Place,
    REDELIVER_AFTER, Refusal,
};
use crate::node_host::NodeHost;
use crate::process_tree;
use crate::profile::codex::{self, Status};
use crate::profile::opencode2;
use crate::profile::{
    self, AgentId, LaunchError, Mailbox, Profile, Remote, ServerLink, Start, VersionMismatch,
};
use crate::settings::Settings;
use crate::worktree::{self, RepoId, RepoProblem, RepoView, Repos, Worktree};

#[derive(Embed)]
#[folder = "web/dist"]
struct Assets;

struct App {
    repos: Repos,
    host: NodeHost<NodeId>,
    graph: Arc<Graph>,
    settings: Settings,
    accounts: Accounts,
    logins: settings::Logins,
    /// Codex 노드마다 그 노드가 붙은 app-server. 노드 프로세스가 끝나면 지우고 끝낸다.
    codex: Mutex<HashMap<NodeId, Arc<CodexServer>>>,
    /// OpenCode 2 노드마다 그 노드의 서버와 세션. 노드 프로세스가 끝나면 지우고 끝낸다.
    opencode2: Mutex<HashMap<NodeId, Arc<opencode2::Node>>>,
    /// 설치한 CLI 버전이 고정 버전과 다르면 설정 화면에 보일 경고. 데몬이 시작할 때 한 번 본다.
    version_warnings: HashMap<Profile, VersionMismatch>,
    /// Codex app-server에 넘길 샌드박스 `-c` 인자. 노드 문이 없거나 버전이 다르면 비어 있다.
    codex_sandbox: Vec<String>,
    /// 노드 안에서 `loom` 명령이 쓰는 노드 문 주소(`LOOM_URL`).
    url: String,
    /// `loom spawn`이 쓰는 데몬 주소(`LOOM_SPAWN_URL`). Codex 샌드박스는 이 주소를 막는다.
    spawn_url: String,
    /// 노드의 `PATH`. 앞에 이 실행 파일의 폴더를 넣어 노드가 `loom`을 바로 부르게 한다.
    path: OsString,
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

/// 에이전트 경로(`/api/agent/*`)의 실패. 몸통은 `loom` 명령이 그대로 찍는 한국어 문장이다.
type AgentError = (StatusCode, String);

/// 화면 경로의 `Problem`을 에이전트가 읽는 문장으로 바꾼다.
fn for_agent(e: impl Into<Problem>) -> AgentError {
    let problem = e.into();
    let text = match &problem {
        Problem::Refused(e) => e.to_string(),
        Problem::Launch(e) => e.to_string(),
        Problem::Repo(e) => e.to_string(),
        Problem::NoLogin => "없는 로그인 창이다.".into(),
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

/// [저장소 추가]. `path`는 데몬이 보는 폴더다.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct NewRepo {
    pub path: String,
}

/// [새 워크트리]. `base`에서 새 브랜치 `branch`를 만든다.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct NewWorktree {
    pub branch: String,
    pub base: String,
}

/// [워크트리 삭제]의 대상.
#[derive(Deserialize)]
pub struct WorktreePath {
    pub path: String,
}

/// [워크트리 삭제]. `delete_branch`가 참이면 그 워크트리의 브랜치도 지운다(삭제 창의 확인란).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteWorktree {
    pub path: String,
    #[serde(default)]
    pub delete_branch: bool,
}

/// 커밋 그래프 칸이 읽는 것: 커밋과, 워크트리마다 체크아웃한 커밋.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct CommitGraph {
    pub commits: Vec<Commit>,
    pub worktrees: Vec<Worktree>,
}

/// PTY WebSocket의 텍스트 프레임. 바이너리 프레임은 키 입력과 터미널 출력이다.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct Resize {
    pub cols: u16,
    pub rows: u16,
}

/// 에이전트 명령(`loom spawn`)의 요청. 응답은 새 노드의 id다.
#[derive(Serialize, Deserialize)]
pub struct SpawnRequest {
    pub name: String,
    pub run: SpawnRun,
    pub brief: String,
    /// 있으면 부모 워크트리의 HEAD에서 이 브랜치로 새 워크트리를 만들고 자식을 그 안에 띄운다.
    pub worktree: Option<String>,
}

/// 자식이 실행할 것.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SpawnRun {
    /// 카탈로그 에이전트. 지시문은 카탈로그의 방법으로 시작 명령에 넘긴다.
    Agent(AgentId),
    /// 셸이 해석하는 명령 한 줄. 지시문은 메일함 첫 메시지로 넣고, 자식이 `loom wait`로 받는다.
    Command(String),
}

/// `loom send`·`steer`·`interrupt`의 요청.
#[derive(Serialize, Deserialize)]
pub struct SendRequest {
    pub to: String,
    pub kind: Kind,
    pub body: String,
}

/// `loom hook`의 요청. `kinds` 가운데 하나인 메시지를 꺼낸다. 본문 합 상한은 `limit_bytes`다(첫 메시지는 늘 꺼낸다).
#[derive(Serialize, Deserialize)]
pub struct TakeRequest {
    pub kinds: Vec<Kind>,
    pub limit_bytes: usize,
}

/// `loom guard`의 요청. `caller`는 `loom guard` 프로세스의 PID다. 끝낼 셸은 데몬이 그 조상에서 고른다.
#[derive(Serialize, Deserialize)]
pub struct GuardRequest {
    pub caller: u32,
}

/// `loom wait`·`loom hook`이 받은 메시지를 확인한다.
#[derive(Serialize, Deserialize)]
pub struct AckRequest {
    pub ids: Vec<MessageId>,
}

/// `loom adopt`의 요청. 부르는 노드가 `name` 노드의 새 부모가 된다.
#[derive(Serialize, Deserialize)]
pub struct AdoptRequest {
    pub name: String,
}

/// `loom done`의 요청. `summary`는 요약이나 결과 파일 경로다.
#[derive(Serialize, Deserialize)]
pub struct DoneRequest {
    pub summary: String,
}

/// 노드 문이 듣는 루프백 주소. Codex 샌드박스는 이 주소 하나만 연다(`codex::sandbox_args`).
/// 다른 서비스가 쓰지 않는 주소를 골라, `127.0.0.1`의 다른 포트는 샌드박스 안에서 계속 막힌다.
const NODE_IP: Ipv4Addr = Ipv4Addr::new(127, 76, 77, 1);

pub async fn serve(listen: SocketAddr) -> std::io::Result<()> {
    process_tree::end_descendants_on_exit()?;
    let listener = tokio::net::TcpListener::bind(listen).await?;
    // 포트 0이면 OS가 고른 포트다. 노드 주소와 시작 줄은 실제 주소를 쓴다. Windows 앱은 시작 줄에서 주소를 읽는다.
    let listen = listener.local_addr()?;
    // 포트는 데몬 포트와 같다. 노드 환경의 `LOOM_URL`은 노드가 끝날 때까지 바뀌지 않으므로, 다시 시작한 데몬도 같은 주소에서 듣는다.
    // 그 포트를 쓸 수 없으면(예: `--listen 0.0.0.0:<포트>`) OS가 고른다. 그때 앞 데몬의 노드는 다시 시작한 뒤 loom 명령이 닿지 않는다.
    // macOS는 `127.0.0.1` 말고 루프백 주소가 기본으로 없다(확인하지 않았다).
    // 그때는 노드 문 없이 데몬 주소를 쓰고, Codex는 loom 명령마다 승인을 묻는다.
    let node_door = match tokio::net::TcpListener::bind((NODE_IP, listen.port())).await {
        Ok(door) => Some(door),
        Err(_) => tokio::net::TcpListener::bind((NODE_IP, 0)).await.ok(),
    };
    let codex_version = codex::installed_version();
    let db_path = db::default_path()?;
    let open = || db::open(&db_path).map_err(std::io::Error::other);
    let repos = Repos::new(open()?);
    let (start, start_path) = repos
        .add_start(&std::env::current_dir()?)
        .await
        .map_err(std::io::Error::other)?;
    let graph = Graph::open(open()?).map_err(std::io::Error::other)?;
    graph
        .settle(&Place {
            repo: start,
            worktree: start_path.to_string_lossy().into_owned(),
        })
        .map_err(std::io::Error::other)?;
    // 근거: docs/design.md 「저장」.
    let pty_dir = db_path.with_file_name("pty");
    let app = Arc::new(App {
        repos,
        host: NodeHost::new(pty_dir.join("nodes")),
        graph: Arc::new(graph),
        settings: Settings::new(open()?),
        accounts: Accounts::new(db_path.with_file_name("accounts")),
        logins: settings::Logins::new(pty_dir.join("logins")),
        codex: Mutex::default(),
        opencode2: Mutex::default(),
        version_warnings: profile::version_warnings(codex_version.as_deref()),
        codex_sandbox: match (&codex_version, &node_door) {
            (Some(version), Some(_)) => codex::sandbox_args(version, NODE_IP.into()),
            _ => Vec::new(),
        },
        url: match &node_door {
            Some(door) => format!("http://{}", door.local_addr()?),
            None => node_url(listen),
        },
        spawn_url: node_url(listen),
        path: node_path()?,
    });
    app.resume().await.map_err(std::io::Error::other)?;
    // 노드 문에는 `spawn`이 없다. `spawn`은 아무 명령이나 띄우므로 Codex에서 사람 승인을 받게 한다.
    let node_routes = Router::new()
        .route("/api/agent/send", post(agent_send))
        .route("/api/agent/wait", post(agent_wait))
        .route("/api/agent/take", post(agent_take))
        .route("/api/agent/ack", post(agent_ack))
        .route("/api/agent/guard", post(agent_guard))
        .route("/api/agent/model", post(agent_model))
        .route("/api/agent/done", post(agent_done))
        .route("/api/agent/adopt", post(agent_adopt));
    let router = node_routes
        .clone()
        .route("/api/repos", get(list_repos).post(add_repo))
        .route(
            "/api/repos/{id}/worktrees",
            post(create_worktree).delete(delete_worktree),
        )
        .route("/api/repos/{id}/worktrees/changes", get(worktree_changes))
        .route("/api/repos/{id}/commits", get(commits))
        .route("/api/nodes", post(create_node))
        .route("/api/agent/spawn", post(agent_spawn))
        .route("/api/nodes/{id}", delete(remove_node))
        .route("/api/nodes/{id}/parent/{parent}", put(adopt_node))
        .route("/api/nodes/{id}/pty", get(pty))
        .route("/api/events", get(events))
        .route("/api/events/log", get(event_log))
        .merge(settings::routes())
        .fallback(asset)
        .with_state(app.clone());
    println!("loom: http://{listen}");
    let main = axum::serve(listener, router).into_future();
    match node_door {
        Some(door) => {
            let node = axum::serve(door, node_routes.with_state(app)).into_future();
            tokio::try_join!(main, node).map(drop)
        }
        None => main.await,
    }
}

/// 노드는 데몬과 같은 기계에서 돈다. 모든 주소에서 듣더라도 루프백으로 부른다.
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

/// 더한 순서다. 첫째는 처음 띄운 데몬의 시작 폴더다.
async fn list_repos(State(app): State<Arc<App>>) -> Result<Json<Vec<RepoView>>, Problem> {
    let mut views = Vec::new();
    for (id, path) in app.repos.list().map_err(internal)? {
        views.push(worktree::view(id, &path).await);
    }
    Ok(Json(views))
}

async fn add_repo(
    State(app): State<Arc<App>>,
    Json(req): Json<NewRepo>,
) -> Result<Json<RepoView>, Problem> {
    let id = app.repos.add(req.path.trim()).await?;
    Ok(Json(worktree::view(id, &app.repos.path(id)?).await))
}

async fn create_worktree(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
    Json(req): Json<NewWorktree>,
) -> Result<Json<RepoView>, Problem> {
    let id = RepoId(id);
    let repo = app.repos.path(id)?;
    let workspace = app.workspace()?;
    let made = worktree::create(&repo, &repo, &workspace, &req.branch, &req.base).await?;
    app.setup(&Place {
        repo: id,
        worktree: made,
    })
    .await?;
    Ok(Json(worktree::view(id, &repo).await))
}

/// 삭제 창이 보일, 지우면 잃는 변경.
async fn worktree_changes(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
    Query(req): Query<WorktreePath>,
) -> Result<Json<Vec<String>>, Problem> {
    let repo = app.repos.path(RepoId(id))?;
    worktree::find(&repo, &req.path).await?;
    Ok(Json(worktree::changes(&req.path).await?))
}

/// 그 묶음의 노드를 끝내고 그래프에서 지운 뒤 폴더를 지운다. 근거: docs/design.md 「저장소와 워크트리」.
/// 노드를 먼저 끝낸다. Windows는 프로세스가 작업 폴더로 쓰는 폴더를 지우지 못한다.
async fn delete_worktree(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
    Query(req): Query<DeleteWorktree>,
) -> Result<Json<RepoView>, Problem> {
    let id = RepoId(id);
    let repo = app.repos.path(id)?;
    if worktree::find(&repo, &req.path).await?.original {
        return Err(RepoProblem::Original.into());
    }
    for node in app.graph.nodes_in(&req.path).map_err(internal)? {
        app.remove(node).map_err(internal)?;
    }
    let branch = worktree::remove(&repo, &req.path).await?;
    if let (true, Some(branch)) = (req.delete_branch, branch) {
        worktree::delete_branch(&repo, &branch).await?;
    }
    Ok(Json(worktree::view(id, &repo).await))
}

async fn commits(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
) -> Result<Json<CommitGraph>, Problem> {
    let repo = app.repos.path(RepoId(id))?;
    Ok(Json(CommitGraph {
        commits: commit_graph::read(&repo).await.map_err(internal)?,
        worktrees: worktree::worktrees(&repo).await,
    }))
}

impl App {
    /// 설정의 워크스페이스 폴더. 없으면 기본값이다.
    fn workspace(&self) -> Result<PathBuf, Problem> {
        let settings = self.settings.get().map_err(internal)?;
        match settings.workspace_dir {
            Some(dir) => Ok(PathBuf::from(dir)),
            None => worktree::default_workspace().map_err(internal),
        }
    }

    /// 부모 없는 노드를 `place`에서 띄운다.
    async fn start_root(self: &Arc<Self>, place: &Place, start: &Start) -> Result<NodeId, Problem> {
        let cwd = std::path::Path::new(&place.worktree);
        let prepared = self.prepare(start, cwd).await?;
        let discard = prepared.discarder();
        let id = self
            .graph
            .add_root(place, start.agent(), start.brief(), |id| {
                self.launch(id, prepared)
            })
            .inspect_err(|_| discard())?;
        Ok(id)
    }

    /// 새 워크트리에서 저장소 설정 스크립트를 명령 노드로 띄운다. 카드가 출력과 종료 코드를 보인다.
    /// 스크립트가 없으면 아무것도 하지 않는다. 근거: docs/design.md 「저장소와 워크트리」.
    async fn setup(self: &Arc<Self>, place: &Place) -> Result<(), Problem> {
        if let Some(line) = worktree::setup_script(std::path::Path::new(&place.worktree)) {
            self.start_root(place, &Start::Command { line }).await?;
        }
        Ok(())
    }

    /// 노드 하나를 그래프에서 지우고 프로세스를 끝낸다.
    fn remove(&self, node: NodeId) -> rusqlite::Result<()> {
        self.graph.remove(node)?;
        self.host.kill(node);
        Ok(())
    }

    /// 노드 프로세스를 띄운다. 끝나면 그래프에 알린다. Codex 노드면 메일함을 그 app-server로 넘기기 시작한다.
    /// 돌려준 값(토큰, 서버 주소)은 그래프가 노드와 함께 남긴다. 노드가 뜨자마자 데몬을 불러도 그래프가 커밋할 때까지 기다린다.
    fn launch(self: &Arc<Self>, id: NodeId, prepared: Prepared) -> Result<Launched, String> {
        let Prepared {
            cmd,
            token,
            profile,
            codex,
            opencode2,
        } = prepared;
        let server = match (&codex, &opencode2) {
            (Some(server), _) => Some(server.link()),
            (_, Some(node)) => Some(node.link()),
            _ => None,
        };
        let on_exit = self.on_exit(id, &codex, &opencode2);
        self.host
            .spawn(id, cmd, self.on_title(id, profile), on_exit)?;
        self.follow_servers(id, codex, opencode2);
        Ok(Launched { token, server })
    }

    /// 앞 데몬이 띄운 `running` 노드의 PTY 호스트와 서버에 다시 붙는다. 데몬이 시작할 때 한 번 부른다.
    /// 호스트가 없으면 노드는 코드 없는 `exited`가 되고, 남은 서버를 끝낸다.
    /// 그 뒤 붙지 않은 호스트(꺼진 동안 지운 노드, 앞 데몬의 로그인 창)를 끝낸다.
    async fn resume(self: &Arc<Self>) -> rusqlite::Result<()> {
        for node in self.graph.running()? {
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
            let on_exit = self.on_exit(node.id, &codex, &opencode2);
            if self
                .host
                .resume(node.id, self.on_title(node.id, profile), on_exit)
                .is_err()
            {
                if let Some(server) = &node.server {
                    stop_server(server);
                }
                self.graph.lost(node.id)?;
                continue;
            }
            self.follow_servers(node.id, codex, opencode2);
        }
        self.host.end_others();
        self.logins.end_others();
        Ok(())
    }

    /// 셸·명령 노드의 제목(`user@host: dir`)은 주제가 아니다. 프로필이 제목 형식을 아는 노드만 쓴다.
    fn on_title(&self, id: NodeId, profile: Option<Profile>) -> impl FnMut(String) + use<> {
        let graph = self.graph.clone();
        move |title: String| {
            if let Some(profile) = profile {
                // DB가 실패하면 알릴 곳이 없다. 화면은 앞 주제로 남는다.
                let _ = graph.set_title(id, profile.topic(&title));
            }
        }
    }

    /// 노드 프로세스가 끝나면 그 서버를 끝내고 그래프에 알린다.
    /// 서버는 맵이 아니라 이 클로저가 쥔다. 다시 붙은 호스트는 붙자마자 종료 코드를 보낼 수 있다.
    fn on_exit(
        self: &Arc<Self>,
        id: NodeId,
        codex: &Option<Arc<CodexServer>>,
        opencode2: &Option<Arc<opencode2::Node>>,
    ) -> impl FnOnce(u32) + use<> {
        let app = self.clone();
        let discard = discarder(codex, opencode2);
        move |code| {
            discard();
            app.codex.lock().unwrap().remove(&id);
            app.opencode2.lock().unwrap().remove(&id);
            // DB가 실패하면 알릴 곳이 없다. 화면은 running으로 남는다.
            let _ = app.graph.exited(id, code);
        }
    }

    /// 노드 서버의 상태를 따라가고 메일함을 넘기기 시작한다.
    fn follow_servers(
        self: &Arc<Self>,
        id: NodeId,
        codex: Option<Arc<CodexServer>>,
        opencode2: Option<Arc<opencode2::Node>>,
    ) {
        if let Some(server) = codex {
            self.codex.lock().unwrap().insert(id, server.clone());
            tokio::spawn(follow_codex_model(self.graph.clone(), id, server.clone()));
            tokio::spawn(deliver_to_codex(self.clone(), id, server));
        }
        if let Some(node) = opencode2 {
            self.opencode2.lock().unwrap().insert(id, node.clone());
            tokio::spawn(follow_model(self.graph.clone(), id, node.model()));
            let mailbox = NodeMailbox {
                graph: self.graph.clone(),
                node: id,
            };
            tokio::spawn(async move { node.run(mailbox).await });
        }
    }

    fn caller(&self, headers: &HeaderMap) -> Result<NodeId, AgentError> {
        let token = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        let node = match token {
            Some(token) => self
                .graph
                .caller(token)
                .map_err(|e| for_agent(Refusal::from(e)))?,
            None => None,
        };
        node.ok_or((
            StatusCode::UNAUTHORIZED,
            "loom 노드 안에서만 부른다.".into(),
        ))
    }

    /// Claude·Codex면 설정에서 고른 계정의 로그인 폴더를 넣는다. Codex면 그 계정으로 app-server를 먼저 띄운다.
    /// OpenCode 2면 노드 서버와 세션을 먼저 띄운다.
    /// `cwd`는 노드가 도는 워크트리다.
    async fn prepare(&self, start: &Start, cwd: &std::path::Path) -> Result<Prepared, Problem> {
        let profile = start.profile();
        let provider = profile.and_then(Profile::provider);
        let settings = self.settings.get().map_err(internal)?;
        let account = provider.and_then(|p| settings.account(p));
        let token = new_token().map_err(internal)?;
        let env = self.node_env(&token);
        let codex = match profile {
            Some(Profile::Codex) => {
                let home = self.accounts.config_dir(profile::Provider::Codex, account);
                let server = CodexServer::start(home.as_deref(), &env, &self.codex_sandbox)
                    .await
                    .map_err(|cause| LaunchError::Server {
                        program: "codex app-server".into(),
                        cause,
                    })?;
                Some(Arc::new(server))
            }
            _ => None,
        };
        let opencode2 = match profile {
            Some(Profile::OpenCode2) => {
                let password = new_token().map_err(internal)?;
                let node = opencode2::Node::start(cwd, &env, password)
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
        let mut cmd = profile::command(start, cwd, remote, &settings.bypass_off)
            .inspect_err(|_| discard())?;
        for (key, value) in &env {
            cmd.env(key, value);
        }
        if let Some(provider) = provider {
            self.accounts.apply(&mut cmd, provider, account);
        }
        Ok(Prepared {
            cmd,
            token,
            profile,
            codex,
            opencode2,
        })
    }

    /// 노드 안의 `loom` 명령이 쓰는 환경. Codex는 셸을 TUI가 아니라 app-server 아래에서 띄우므로 둘 다 받는다.
    fn node_env(&self, token: &str) -> [(&'static str, OsString); 4] {
        [
            ("LOOM_URL", self.url.clone().into()),
            ("LOOM_SPAWN_URL", self.spawn_url.clone().into()),
            ("LOOM_TOKEN", token.into()),
            ("PATH", self.path.clone()),
        ]
    }
}

/// 띄울 준비가 된 노드. Codex·OpenCode 2 노드면 TUI가 붙을 서버가 이미 떠 있다.
struct Prepared {
    cmd: portable_pty::CommandBuilder,
    /// 노드가 데몬을 부를 때 내는 토큰. 그래프가 노드와 함께 남긴다.
    token: String,
    /// 터미널 제목에서 카드 주제를 꺼내는 프로필. 없으면 제목을 쓰지 않는다.
    profile: Option<Profile>,
    codex: Option<Arc<CodexServer>>,
    opencode2: Option<Arc<opencode2::Node>>,
}

impl Prepared {
    /// 노드를 만들지 못했으면 미리 띄운 서버를 끝내는 함수. 노드를 띄우는 클로저가 `self`를 가져가므로 따로 만든다.
    fn discarder(&self) -> impl Fn() + use<> {
        discarder(&self.codex, &self.opencode2)
    }
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

/// 그래프에 있는 노드 하나의 메일함. 프로필의 전달 작업에 넘긴다.
struct NodeMailbox {
    graph: Arc<Graph>,
    node: NodeId,
}

impl Mailbox for NodeMailbox {
    fn changed(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        let mut changed = Box::pin(self.graph.mail_changed());
        changed.as_mut().enable();
        // 확인 없이 둔 메시지는 새 메시지 알림 없이 다시 꺼낼 수 있게 된다.
        Box::pin(async move {
            let _ = tokio::time::timeout(REDELIVER_AFTER, changed).await;
        })
    }

    fn take(&self, kinds: &[Kind], limit_bytes: usize) -> Result<Vec<Delivery>, String> {
        self.graph
            .take(self.node, kinds, limit_bytes)
            .map_err(|e| e.to_string())
    }

    fn ack(&self, taken: &[Delivery]) -> Result<(), String> {
        let ids: Vec<MessageId> = taken.iter().map(|d| d.id).collect();
        self.graph.ack(self.node, &ids).map_err(|e| e.to_string())
    }
}

/// 프로필이 읽은 모델과 추론 강도를 그래프에 옮긴다. 서버가 끝나면 끝난다.
async fn follow_model(graph: Arc<Graph>, node: NodeId, mut model: watch::Receiver<Model>) {
    while model.changed().await.is_ok() {
        let now = model.borrow_and_update().clone();
        if let Err(e) = graph.set_model(node, &now) {
            eprintln!("loom: 노드 {}의 모델을 쓰지 못했다: {e}", node.0);
        }
    }
}

/// Codex 노드 스레드가 생기거나 상태가 바뀔 때마다 모델을 다시 읽어 그래프에 옮긴다. 서버가 끝나면 끝난다.
/// 스레드가 새로 생기면 다시 붙는 값(`ServerLink`)도 그래프에 남긴다. 다시 시작한 데몬은 `thread/started`를 다시 받지 못한다.
async fn follow_codex_model(graph: Arc<Graph>, node: NodeId, server: Arc<CodexServer>) {
    let mut thread = server.thread();
    let mut kept = thread.borrow().as_ref().map(|t| t.id.clone());
    while thread.changed().await.is_ok() {
        let Some(id) = thread.borrow_and_update().as_ref().map(|t| t.id.clone()) else {
            continue;
        };
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

/// Codex 노드의 메일함을 app-server로 넘긴다. 노드마다 이 작업 하나만 꺼내므로 같은 메시지를 두 번 넘기지 않는다.
/// 받는 노드의 상태에 따라 꺼내는 방식을 고른다. 근거: docs/design.md 「전달 지점」.
async fn deliver_to_codex(app: Arc<App>, node: NodeId, server: Arc<CodexServer>) {
    let mut thread = server.thread();
    // `turn/start`를 보낸 뒤 서버의 active 알림이 오기 전까지 일하는 중으로 본다. 그래야 큐를 두 턴으로 나눠 보내지 않는다.
    let mut started = false;
    loop {
        let mut mail = pin!(app.graph.mail_changed());
        mail.as_mut().enable();
        let now = thread.borrow_and_update().clone();
        if let Some(now) = now {
            let status = if started { Status::Busy } else { now.status };
            match app
                .graph
                .take(node, codex::kinds(status), codex::INPUT_LIMIT_BYTES)
            {
                Ok(taken) if !taken.is_empty() => {
                    let call = |method, params| server.call(method, params);
                    match codex::deliver(call, &now.id, status, &taken).await {
                        Ok(()) => {
                            started = true;
                            let ids: Vec<MessageId> = taken.iter().map(|d| d.id).collect();
                            if let Err(e) = app.graph.ack(node, &ids) {
                                eprintln!("loom: 노드 {}의 받음 확인을 쓰지 못했다: {e}", node.0);
                            }
                        }
                        // 확인하지 않았으므로 `REDELIVER_AFTER` 뒤에 다시 꺼낸다.
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
            // 넘기지 못한 메시지는 새 메시지 알림 없이 다시 꺼낼 수 있게 된다.
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

/// 다시 붙지 못한 노드의 서버를 끝낸다. 서버가 이미 끝났으면 할 일이 없다.
/// PID를 다른 프로세스가 받았을 수 있다. 데몬이 꺼진 동안 노드가 끝난 드문 경우다.
fn stop_server(server: &ServerLink) {
    let (ServerLink::Codex { pid, .. } | ServerLink::OpenCode2 { pid, .. }) = server;
    if let Some(root) = pid {
        process_tree::Snapshot::now().kill_trees([*root]);
    }
}

/// 노드 토큰과 OpenCode 2 서버 비밀번호. 128비트 무작위 값의 16진수다.
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

/// 노드 하나를 지우고 프로세스를 끝낸다. 프로세스가 끝나면 `launch`의 `on_exit`가 app-server를 끝낸다.
/// 그래프에서는 이미 지웠으므로 `on_exit`의 `exited`는 아무것도 바꾸지 않는다.
async fn remove_node(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
) -> Result<Json<()>, Problem> {
    app.remove(NodeId(id)).map_err(internal)?;
    Ok(Json(()))
}

/// 사람이 화면에서 `parent`를 `id`의 새 부모로 정한다.
async fn adopt_node(
    State(app): State<Arc<App>>,
    Path((id, parent)): Path<(u32, u32)>,
) -> Result<Json<()>, Problem> {
    app.graph.adopt(NodeId(parent), NodeId(id))?;
    Ok(Json(()))
}

async fn agent_spawn(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<SpawnRequest>,
) -> Result<Json<NodeId>, AgentError> {
    let parent = app.caller(&headers)?;
    let (start, queued_brief) = match req.run {
        SpawnRun::Agent(agent) => (
            Start::Agent {
                agent,
                brief: req.brief,
            },
            String::new(),
        ),
        SpawnRun::Command(line) => (Start::Command { line }, req.brief),
    };
    let place = app
        .graph
        .place_of(parent)
        .map_err(|e| for_agent(Refusal::from(e)))?;
    // 새 워크트리는 자식을 띄우지 못해도 남는다. 깨끗한 워크트리라 사람이 [워크트리 닫기]로 닫는다.
    let worktree = match &req.worktree {
        Some(branch) => {
            let repo = app.repos.path(place.repo).map_err(for_agent)?;
            let workspace = app.workspace().map_err(for_agent)?;
            let from = std::path::Path::new(&place.worktree);
            let made = worktree::create(&repo, from, &workspace, branch, "HEAD")
                .await
                .map_err(for_agent)?;
            app.setup(&Place {
                repo: place.repo,
                worktree: made.clone(),
            })
            .await
            .map_err(for_agent)?;
            Some(made)
        }
        None => None,
    };
    let cwd = std::path::Path::new(worktree.as_deref().unwrap_or(&place.worktree));
    let prepared = app.prepare(&start, cwd).await.map_err(for_agent)?;
    let discard = prepared.discarder();
    let id = app
        .graph
        .add_child(
            parent,
            &req.name,
            worktree.as_deref(),
            start.agent(),
            &queued_brief,
            |id| app.launch(id, prepared),
        )
        .map_err(for_agent)
        .inspect_err(|_| discard())?;
    Ok(Json(id))
}

async fn agent_send(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<SendRequest>,
) -> Result<Json<Option<NodeId>>, AgentError> {
    let from = app.caller(&headers)?;
    let sent = send(&app, from, &req).await;
    // 거절은 보낸 노드만 본다. 사람이 이벤트 콘솔에서 보게 남긴다. DB 오류는 요청의 잘못이 아니므로 남기지 않는다.
    if let Err(reason) = &sent
        && !matches!(reason, Refusal::Db(_))
    {
        app.graph
            .record_refusal(from, &req.to, req.kind, reason)
            .map_err(|e| for_agent(Refusal::from(e)))?;
    }
    sent.map(Json).map_err(for_agent)
}

async fn send(app: &App, from: NodeId, req: &SendRequest) -> Result<Option<NodeId>, Refusal> {
    if req.kind != Kind::Queue {
        let (to, agent) = app.graph.recipient(from, &req.to)?;
        // 조정·중단은 프로필(훅·app-server)이 있는 노드만 받는다. 큐로 바꿔 보내지 않는다. 근거: docs/design.md 「명령」.
        let Some(profile) = agent.as_ref().and_then(profile::profile_of) else {
            return Err(Refusal::NotSteerable(req.to.clone()));
        };
        // 셸을 먼저 끝낸다. 메시지를 먼저 넣으면 그 셸 안의 `loom wait`가 본문을 꺼낸 뒤 죽어 본문을 잃을 수 있다.
        if req.kind == Kind::Interrupt {
            match profile {
                Profile::Claude => {
                    app.host.stop_guarded(to);
                }
                Profile::Codex => {
                    let server = app.codex.lock().unwrap().get(&to).cloned();
                    if let Some(server) = server {
                        server.stop_shells();
                    }
                }
                Profile::OpenCode2 => {
                    let node = app.opencode2.lock().unwrap().get(&to).cloned();
                    if let Some(node) = node {
                        node.stop_shells().await;
                    }
                }
            }
        }
    }
    app.graph.send(from, &req.to, req.kind, &req.body)
}

async fn agent_take(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<TakeRequest>,
) -> Result<Json<Vec<Delivery>>, AgentError> {
    let node = app.caller(&headers)?;
    let taken = app.graph.take(node, &req.kinds, req.limit_bytes);
    Ok(Json(taken.map_err(for_agent)?))
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

/// `loom hook`이 Claude 훅 입력에서 읽은 모델과 추론 강도.
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

async fn agent_wait(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
) -> Result<Json<Delivery>, AgentError> {
    let node = app.caller(&headers)?;
    Ok(Json(app.graph.wait(node).await.map_err(for_agent)?))
}

async fn agent_done(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<DoneRequest>,
) -> Result<Json<()>, AgentError> {
    let node = app.caller(&headers)?;
    app.graph.done(node, &req.summary).map_err(for_agent)?;
    Ok(Json(()))
}

async fn agent_adopt(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<AdoptRequest>,
) -> Result<Json<()>, AgentError> {
    let parent = app.caller(&headers)?;
    let child = app.graph.find(&req.name).map_err(for_agent)?;
    app.graph.adopt(parent, child).map_err(for_agent)?;
    Ok(Json(()))
}

async fn pty(ws: WebSocketUpgrade, Path(id): Path<u32>, State(app): State<Arc<App>>) -> Response {
    serve_pty(ws, app, |app| &app.host, NodeId(id))
}

/// PTY 웹소켓의 글 프레임. 터미널 출력은 바이너리 프레임으로 간다.
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PtyNotice {
    /// 이 PTY가 없다. 데몬이 다시 시작할 때 호스트가 사라졌거나, 앞 데몬에서 끝난 노드다.
    Gone,
}

/// PTY 하나를 화면에 잇는다. 노드 터미널과 로그인 창이 함께 쓴다.
fn serve_pty<K: Copy + Eq + Hash + std::fmt::Display + Send + Sync + 'static>(
    ws: WebSocketUpgrade,
    app: Arc<App>,
    host: fn(&App) -> &NodeHost<K>,
    id: K,
) -> Response {
    let Some(attached) = host(&app).attach(id) else {
        // PTY가 없다. 화면이 안내를 보이게 알리고, 다시 붙지 않게 연결을 열어 둔다.
        return ws.on_upgrade(|mut socket| async move {
            let notice =
                serde_json::to_string(&PtyNotice::Gone).expect("PtyNotice는 늘 직렬화된다");
            if socket.send(Message::Text(notice.into())).await.is_ok() {
                while let Some(Ok(_)) = socket.next().await {}
            }
        });
    };
    ws.on_upgrade(move |socket| async move {
        let (mut to_view, mut from_view) = socket.split();
        let mut output = attached.output;
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
                    // 밀린 화면은 끊는다. 화면이 다시 붙으면 backlog로 따라잡는다.
                    Err(RecvError::Lagged(_) | RecvError::Closed) => return,
                },
                msg = from_view.next() => match msg {
                    Some(Ok(Message::Binary(bytes))) => host(&app).write(id, &bytes),
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(Resize { cols, rows }) = serde_json::from_str(&text) {
                            host(&app).resize(id, cols, rows);
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => return,
                },
            }
        }
    })
}

async fn event_log(State(app): State<Arc<App>>) -> Result<Json<Vec<EventRecord>>, Problem> {
    app.graph.recent_events().map(Json).map_err(internal)
}

async fn events(ws: WebSocketUpgrade, State(app): State<Arc<App>>) -> Response {
    ws.on_upgrade(move |socket| send_events(socket, app))
}

async fn send_events(mut socket: WebSocket, app: Arc<App>) {
    let Ok((snapshot, mut rx)) = app.graph.subscribe() else {
        return;
    };
    for view in snapshot {
        if send_json(&mut socket, &GraphEvent::Node(view))
            .await
            .is_err()
        {
            return;
        }
    }
    // 밀리거나 끊기면 닫는다. 화면이 다시 붙으면 목록 전체를 다시 받는다.
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

/// 첫 페이지(`/`)는 테마를 넣어 보내는 `settings::index`가 맡는다.
async fn asset(uri: Uri) -> Response {
    match Assets::get(uri.path().trim_start_matches('/')) {
        Some(file) => (
            [(header::CONTENT_TYPE, file.metadata.mimetype().to_owned())],
            file.data,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 화면(`web/src/problem.ts`)이 읽는 모양이다. 바뀌면 화면 번역도 함께 고친다.
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
            (StatusCode::BAD_REQUEST, "이름을 적는다.".into())
        );
    }
}
