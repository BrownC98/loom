//! 조합 레이어: HTTP·WebSocket 라우트. 아래 기능 모듈을 부르고 결과를 엮기만 한다.
//! 설정·계정·사용량 라우트는 `settings.rs`에 둔다.

mod carry;
mod idle;
mod origin;
mod phone;
mod places;
mod relaunch;
mod sessions;
mod settings;
pub use relaunch::Relaunched;
mod switch;
mod turn_end;

use origin::ViaServe;

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
    Delivery, EventRecord, Graph, GraphEvent, Kind, Launched, Listed, MessageId, Model, NewChild,
    NodeId, Place, REDELIVER_AFTER, Refusal, Relations, Sent, TreeLimits, WaitDoc, WaitId,
    WaitKind, WaitSource,
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
use crate::screen;
use crate::settings::Settings;
use crate::skills::{self, Launch, Position};
use crate::spots::{Arranged, CardSize, Layout, Placed, Spots};
use crate::update::{UpdateView, Updater};
use crate::user_doc;
use crate::worktree::{self, FolderList, RepoId, RepoProblem, RepoView, Repos, Worktree};
use crate::wsl;

#[derive(Embed)]
#[folder = "web/dist"]
struct Assets;

struct App {
    /// 데몬의 DB 연결. 옮기기(`carry`)만 직접 쓴다. 다른 모듈은 자기 복사본(`Repos`, `Graph` 등)으로 쓴다.
    database: db::Database,
    repos: Repos,
    host: NodeHost<NodeId>,
    graph: Arc<Graph>,
    settings: Settings,
    spots: Spots,
    /// 이 PC의 loom 계정 폴더. WSL 배포판의 계정은 그 배포판에 있다(`accounts::Site`).
    accounts_root: PathBuf,
    logins: settings::Logins,
    /// Codex 노드마다 그 노드가 붙은 app-server. 노드 프로세스가 끝나면 지우고 끝낸다.
    codex: Mutex<HashMap<NodeId, Arc<CodexServer>>>,
    /// OpenCode 2 노드마다 그 노드의 서버와 세션. 노드 프로세스가 끝나면 지우고 끝낸다.
    opencode2: Mutex<HashMap<NodeId, Arc<opencode2::Node>>>,
    /// 노드마다 런타임의 세션 단계. 프로세스를 띄울 때마다 `follow_screen`이 새로 연다.
    sessions: sessions::Sessions,
    /// 끝나지 않은 설정 스크립트 노드. 성공하면 출력이 있어도 지운다. 근거: docs/design.md 「저장소와 워크트리」.
    setups: Mutex<HashSet<NodeId>>,
    /// 쉬는 Claude 노드마다 메시지를 기다리는 비동기 훅 하나.
    idle: idle::IdleWaits,
    /// 다시 띄우는 중이거나 턴 끝에 다시 띄울 노드.
    relaunches: relaunch::Relaunches,
    /// 자동 계정 전환 기록.
    switch: switch::Switches,
    /// 턴 끝 질문 검사(`turn_end`).
    turn_ends: turn_end::TurnEnds,
    /// 노드 에이전트가 쓰는 내장 스킬. 데몬이 시작할 때 데이터 폴더에 푼다.
    plugin: skills::Plugin,
    /// WSL 노드의 `loom` 껍데기 폴더(`<데이터 폴더>/wsl`). Windows 데몬만 쓴다.
    shim: PathBuf,
    /// WSL 배포판마다 홈. 저장소 경로를 `~/…`로 보이고 기본 워크스페이스를 정한다.
    homes: wsl::Homes,
    /// 설치한 CLI 버전이 고정 버전과 다르면 설정 화면에 보일 경고. 데몬이 시작할 때 한 번 본다.
    version_warnings: HashMap<Profile, VersionMismatch>,
    /// Codex app-server에 넘길 샌드박스 `-c` 인자. 노드 문이 없거나 버전이 다르면 없다. 사용법 지시는 노드마다 더한다.
    codex_config: Vec<String>,
    /// 노드 안에서 `loom` 명령이 쓰는 노드 문 주소(`LOOM_URL`).
    url: String,
    /// `loom spawn`이 쓰는 데몬 주소(`LOOM_SPAWN_URL`). Codex 샌드박스는 이 주소를 막는다.
    spawn_url: String,
    /// 노드의 `PATH`. 앞에 이 실행 파일의 폴더를 넣어 노드가 `loom`을 바로 부르게 한다.
    path: OsString,
    update: Arc<Updater>,
    /// 짝지은 폰. 사람 문(`origin::human_site`)도 같은 값을 쓴다.
    devices: Arc<Devices>,
    /// 사람 문이 듣는 포트. Tailscale Serve가 이 포트로 넘긴다.
    port: u16,
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

/// 붙여넣은 파일을 받을 터미널의 노드. 로그인 창이면 없다.
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

/// [저장소 추가]의 폴더 목록. 없으면 `<홈>/loom`이다.
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

/// [워크트리 삭제]의 대상.
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

/// PTY 크기. 화면이 보내면 PTY 크기를 바꾸고, 데몬은 바뀐 크기를 모든 화면에 알린다(`PtyNotice::Size`).
/// 바이너리 프레임은 키 입력과 터미널 출력이다.
#[derive(Clone, Copy, Serialize, Deserialize, TS)]
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
    /// `SpawnRun::Agent`에만 쓴다. 옛 CLI는 보내지 않으므로 없으면 빈 값이다.
    #[serde(default)]
    pub choice: ModelChoice,
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

#[derive(Serialize, Deserialize)]
pub struct ReplyRequest {
    pub message: MessageId,
    pub body: String,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WaitRequest {
    pub reply_to: Option<MessageId>,
    pub timeout_secs: Option<u64>,
}

/// `loom hook`의 요청. `kinds` 가운데 하나인 메시지를 꺼낸다. 본문 합 상한은 `limit_bytes`다(첫 메시지는 늘 꺼낸다).
#[derive(Serialize, Deserialize)]
pub struct TakeRequest {
    pub kinds: Vec<Kind>,
    pub limit_bytes: usize,
    /// 훅 입력의 `transcript_path`(Claude·Cursor 세션 기록 파일). 다시 띄우기가 쓴다. 옛 `loom hook`은 보내지 않는다.
    #[serde(default)]
    pub transcript: Option<String>,
    /// 사람이 입력한 턴의 첫 훅이다. 열린 `loom ask` 질문을 알린다(`Taken::open_question`).
    #[serde(default)]
    pub human_input: bool,
}

/// `loom hook`의 응답. 꺼낸 메시지.
#[derive(Serialize, Deserialize)]
pub struct Taken {
    pub messages: Vec<Delivery>,
    /// 사람 입력 훅에서만: 그 입력에 붙일 안내(`App::human_input`).
    #[serde(default)]
    pub note: Option<String>,
}

/// 쉬는 노드의 훅 요청(`loom hook`, `loom cursor-hook`). 옛 `loom hook`은 몸통 없이(`null`) 부른다.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct IdleRequest {
    /// 훅 입력의 `transcript_path`. 다시 띄우기가 쓴다.
    pub transcript: Option<String>,
    /// 계정 한도로 턴이 끝났다(`StopFailure`의 `rate_limit`).
    pub rate_limited: bool,
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

/// `loom rm`의 요청. 부르는 노드가 자기 자식 `name`(이름이나 `#번호`)을 지운다.
#[derive(Serialize, Deserialize)]
pub struct RemoveRequest {
    pub name: String,
}

/// `loom relaunch`의 요청. `name`이 없으면 부르는 노드 자신이다. 있으면 그 자식이다.
#[derive(Serialize, Deserialize)]
pub struct RelaunchRequest {
    pub name: Option<String>,
}

/// `loom ask`의 요청. 질문 한 줄이다.
#[derive(Serialize, Deserialize)]
pub struct AskRequest {
    pub question: String,
    /// `--doc`: 질문에 붙일 user-doc 문서의 절대 경로.
    #[serde(default)]
    pub doc: Option<PathBuf>,
}

/// `loom ask`의 답: 질문 번호, 붙은 문서, 이 질문을 더한 뒤 열린 질문 번호들.
#[derive(Serialize, Deserialize)]
pub struct Asked {
    pub id: WaitId,
    pub doc: Option<WaitDoc>,
    #[serde(default)]
    pub open: Vec<WaitId>,
}

/// `loom answered`: 사람의 입력이 답한 질문 번호들. 비면 열린 질문이 하나일 때만 그것이다.
#[derive(Default, Serialize, Deserialize)]
pub struct AnsweredRequest {
    #[serde(default)]
    pub ids: Vec<WaitId>,
}

/// 사람이 화면에서 `loom ask` 질문에 쓴 답. `images`는 답에 붙인 그림의 데몬 기계 경로다(`POST /api/pastes`가 준 값, 목업 M30).
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct AnswerWait {
    pub text: String,
    #[serde(default)]
    pub images: Vec<PathBuf>,
}

/// 에이전트 답 글(Claude 기록 파일의 마지막 답, Cursor `afterAgentResponse`). 턴 끝 질문 검사가 쓴다.
#[derive(Serialize, Deserialize)]
pub struct SaidRequest {
    pub text: String,
}

/// Claude 훅(`loom hook`)이 알린 사람 응답 대기. `None`이면 훅이 건 대기를 거둔다.
#[derive(Serialize, Deserialize)]
pub struct HookWaitRequest {
    pub wait: Option<(WaitKind, String)>,
}

/// `loom done`의 요청. `summary`는 요약이나 결과 파일 경로다.
#[derive(Serialize, Deserialize)]
pub struct DoneRequest {
    pub summary: String,
    /// `--asked`: 그 일을 맡긴 사람의 지시. 부모 없는 노드는 꼭 준다(받은편지함 완료 줄).
    #[serde(default)]
    pub asked: Option<String>,
    /// `--image`: 사람이 그 지시에 준 그림 파일의 절대 경로.
    #[serde(default)]
    pub images: Vec<PathBuf>,
}

/// 노드 문이 듣는 루프백 주소. Codex 샌드박스는 이 주소 하나만 연다(`codex::sandbox_args`).
/// 다른 서비스가 쓰지 않는 주소를 골라, `127.0.0.1`의 다른 포트는 샌드박스 안에서 계속 막힌다.
/// 화면에서 확인 창을 찾기 전에 출력이 멎어 있어야 하는 시간. TUI는 창 하나를 수십 ms 안에 다 그린다.
/// 일하는 에이전트는 진행 표시를 계속 그리므로 그동안은 찾지 않는다.
const SCREEN_QUIET: Duration = Duration::from_millis(500);

const NODE_IP: Ipv4Addr = Ipv4Addr::new(127, 76, 77, 1);

pub async fn serve(listen: SocketAddr) -> std::io::Result<()> {
    // 데이터 폴더마다 데몬 하나. 둘이 한 DB와 노드를 쥐지 않게 주소보다 먼저 잡고 끝날 때까지 쥔다.
    let claim = launch::Claim::take()?;
    let listener = tokio::net::TcpListener::bind(listen).await?;
    // 포트 0이면 OS가 고른 포트다. 노드 주소, 시작 줄, `daemon.json`은 실제 주소를 쓴다.
    let listen = listener.local_addr()?;
    // 포트는 데몬 포트와 같다. 노드 환경의 `LOOM_URL`은 노드가 끝날 때까지 바뀌지 않으므로, 다시 시작한 데몬도 같은 주소에서 듣는다.
    // 그 포트를 쓸 수 없으면(예: `--listen 0.0.0.0:<포트>`) OS가 고른다. 그때 앞 데몬의 노드는 다시 시작한 뒤 loom 명령이 닿지 않는다.
    // macOS는 `127.0.0.1` 말고 루프백 주소가 기본으로 없다(2026-10-21 macos-14 실측).
    // 그때는 노드 문 없이 데몬 주소를 쓰고, Codex는 loom 명령마다 승인을 묻는다.
    let node_door = match tokio::net::TcpListener::bind((NODE_IP, listen.port())).await {
        Ok(door) => Some(door),
        Err(_) => tokio::net::TcpListener::bind((NODE_IP, 0)).await.ok(),
    };
    let codex_version = codex::installed_version();
    let db_path = db::default_path()?;
    let database = db::open(&db_path)?;
    let repos = Repos::new(database.clone());
    // 시작 폴더가 git 저장소면 더한다. 아니면 더하지 않는다: `loom start`는 루트(`/`)에서 띄우고, 사람은 화면에서 저장소를 더한다.
    match repos.add(&std::env::current_dir()?.to_string_lossy()).await {
        Ok(_) | Err(RepoProblem::NotGit(_) | RepoProblem::NotFound(_)) => {}
        Err(problem) => return Err(std::io::Error::other(problem.to_string())),
    }
    let graph = Graph::open(database.clone()).map_err(std::io::Error::other)?;
    // 옛 노드(DB 9단계 전)는 저장소 목록 첫째로 옮긴다. 저장소가 없으면 그대로 둔다.
    if let Some((repo, path)) = repos
        .list()
        .map_err(std::io::Error::other)?
        .into_iter()
        .next()
    {
        graph
            .settle(&Place {
                repo,
                worktree: path.to_string_lossy().into_owned(),
            })
            .map_err(std::io::Error::other)?;
    }
    // 근거: docs/design.md 「저장」.
    let pty_dir = db_path.with_file_name("pty");
    let graph = Arc::new(graph);
    let update = Arc::new(Updater::new(listen, {
        let graph = graph.clone();
        move || graph.publish_view_changed()
    }));
    let app = Arc::new(App {
        repos,
        host: NodeHost::new(pty_dir.join("nodes")),
        graph,
        settings: Settings::new(database.clone()),
        devices: Arc::new(Devices::new(database.clone())),
        port: listen.port(),
        spots: Spots::new(database.clone()),
        database,
        accounts_root: db_path.with_file_name("accounts"),
        logins: settings::Logins::new(pty_dir.join("logins")),
        codex: Mutex::default(),
        opencode2: Mutex::default(),
        sessions: sessions::Sessions::default(),
        setups: Mutex::default(),
        idle: idle::IdleWaits::default(),
        relaunches: relaunch::Relaunches::default(),
        switch: switch::Switches::default(),
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
        version_warnings: profile::version_warnings(codex_version.as_deref()),
        codex_config: match (&codex_version, &node_door) {
            (Some(version), Some(_)) => codex::sandbox_args(version, NODE_IP.into()),
            _ => Vec::new(),
        },
        url: match &node_door {
            Some(door) => format!("http://{}", door.local_addr()?),
            None => node_url(listen),
        },
        spawn_url: node_url(listen),
        path: node_path()?,
        update: update.clone(),
    });
    app.resume().await.map_err(std::io::Error::other)?;
    // 노드 문에는 `spawn`이 없다. `spawn`은 아무 명령이나 띄우므로 Codex에서 사람 승인을 받게 한다.
    let node_routes = Router::new()
        .route("/api/agent/send", post(agent_send))
        .route("/api/agent/wait", post(agent_wait))
        .route("/api/agent/reply", post(agent_reply))
        .route("/api/agent/status", post(agent_status))
        .route("/api/agent/nodes", post(agent_nodes))
        .route("/api/agent/take", post(agent_take))
        .route("/api/agent/idle", post(agent_idle))
        .route("/api/agent/ack", post(agent_ack))
        .route("/api/agent/guard", post(agent_guard))
        .route("/api/agent/model", post(agent_model))
        .route("/api/agent/done", post(agent_done))
        .route("/api/agent/ask", post(agent_ask))
        .route("/api/agent/hook-wait", post(agent_hook_wait))
        .route("/api/agent/answered", post(agent_answered))
        .route("/api/agent/said", post(agent_said))
        .route("/api/agent/turn-end", post(agent_turn_end))
        .route("/api/agent/adopt", post(agent_adopt))
        .route("/api/agent/rm", post(agent_remove))
        .route("/api/agent/relaunch", post(relaunch::agent_relaunch));
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
        .route("/api/waits/{wait}", delete(dismiss_wait))
        .route("/api/waits/{wait}/answer", post(answer_wait))
        .route("/api/waits/{wait}/doc", post(open_wait_doc))
        .route("/api/waits/{wait}/doc/", get(wait_doc))
        .route("/api/waits/{wait}/doc/{*file}", get(wait_doc_file))
        .route("/api/waits/{wait}/images/{index}", get(done_image_file))
        .route("/api/nodes/{id}/pty", get(pty))
        .route(
            "/api/pastes/{name}",
            post(save_paste).layer(DefaultBodyLimit::max(paste::LIMIT_BYTES)),
        )
        .route("/api/files/open", post(open_file))
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
        .merge(relaunch::routes())
        .merge(carry::routes())
        .fallback(asset)
        .layer(axum::middleware::from_fn_with_state(
            app.devices.clone(),
            origin::human_site,
        ))
        .with_state(app.clone());
    claim.announce(&node_url(listen))?;
    tokio::spawn(update.run());
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

/// 저장소를 목록에서 뺀다. 폴더는 그대로다. 근거: docs/design.md 「저장소와 워크트리」.
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

/// 삭제 창이 보일, 지우면 잃는 변경.
async fn worktree_changes(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
    Query(req): Query<WorktreePath>,
) -> Result<Json<Vec<String>>, Problem> {
    let repo = app.repos.path(RepoId(id))?;
    // git이 적지 않은 경로(이미 지운 워크트리)에는 잃을 것이 없다. 그 경로에서 git을 돌리지 않는다.
    match worktree::find(&repo, &req.path).await {
        Ok(listed) => Ok(Json(worktree::changes(&listed).await?)),
        Err(_) => Ok(Json(Vec::new())),
    }
}

/// 브랜치 이름만 바꾼다. 폴더와 노드는 그대로다.
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

/// 그 묶음의 노드를 끝내고 그래프에서 지운 뒤 폴더를 지운다. 근거: docs/design.md 「저장소와 워크트리」.
/// 노드를 먼저 끝낸다. Windows는 프로세스가 작업 폴더로 쓰는 폴더를 지우지 못한다.
/// git이 적지 않은 경로(loom 밖에서 지운 워크트리)는 노드만 지운다. 브랜치는 짐작하지 않는다.
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

/// 놓은 상자 하나와 그 상자가 민 상자들. 한 번 놓기가 한 요청이다.
async fn put_spot(
    State(app): State<Arc<App>>,
    Json(placed): Json<Vec<Placed>>,
) -> Result<Json<()>, Problem> {
    app.spots.put(&placed).map_err(internal)?;
    app.graph.publish_view_changed();
    Ok(Json(()))
}

/// [정렬]이 구한 자리, 정렬한 저장소의 새 자동 배치.
async fn put_arranged(
    State(app): State<Arc<App>>,
    Json(arranged): Json<Arranged>,
) -> Result<Json<()>, Problem> {
    app.spots.put_arranged(&arranged).map_err(internal)?;
    app.graph.publish_view_changed();
    Ok(Json(()))
}

/// [정렬]: 그 저장소의 자리를 지운다.
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

/// [기본 크기로 되돌리기]와 [모든 카드 기본 크기로].
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
) -> Result<Json<CommitGraph>, Problem> {
    let repo = app.repos.path(RepoId(id))?;
    let commits = match commit_graph::read(&repo).await {
        Ok(commits) => commits,
        // 목록에 넣은 뒤 폴더나 `.git`이 없어졌으면 그 까닭을 보인다. 그 밖은 git의 글 그대로다.
        Err(e) => {
            return Err(worktree::check(&repo)
                .await
                .err()
                .map_or_else(|| internal(e), Into::into));
        }
    };
    Ok(Json(CommitGraph {
        commits,
        worktrees: worktree::worktrees(&repo).await,
    }))
}

impl App {
    fn tree_limits(&self) -> Result<TreeLimits, Problem> {
        Ok(self.settings.get().map_err(internal)?.tree_limits)
    }

    /// 부모 없는 노드를 `place`에서 띄운다.
    async fn start_root(self: &Arc<Self>, place: &Place, start: &Start) -> Result<NodeId, Problem> {
        let cwd = std::path::Path::new(&place.worktree);
        let prepared = self.prepare(start, cwd, Position::Root).await?;
        let discard = prepared.discarder();
        let id = self
            .graph
            .add_root(
                place,
                start.agent(),
                start.brief(),
                &prepared.launched(),
                |id| self.launch(id, prepared),
            )
            .inspect_err(|_| discard())?;
        Ok(id)
    }

    /// [새 워크트리]가 만든 워크트리에서 저장소 설정 스크립트를 명령 노드로 띄운다. 카드가 출력과 종료 코드를 보인다.
    /// 성공하면 노드를 지운다. 스크립트가 없으면 아무것도 하지 않는다. 근거: docs/design.md 「저장소와 워크트리」.
    async fn setup(self: &Arc<Self>, place: &Place) -> Result<(), Problem> {
        let cwd = std::path::Path::new(&place.worktree);
        let Some(line) = worktree::setup_script(cwd) else {
            return Ok(());
        };
        let start = Start::Command { line };
        let prepared = self.prepare(&start, cwd, Position::Root).await?;
        let discard = prepared.discarder();
        self.graph
            .add_root(
                place,
                start.agent(),
                start.brief(),
                &prepared.launched(),
                |id| {
                    // 스크립트가 바로 끝나도 종료 처리가 이 표시를 보도록 띄우기 전에 넣는다.
                    self.setups.lock().unwrap().insert(id);
                    self.launch(id, prepared)
                },
            )
            .inspect_err(|_| discard())?;
        Ok(())
    }

    /// 노드 하나를 그래프에서 지우고 프로세스를 끝낸다.
    fn remove(&self, node: NodeId) -> rusqlite::Result<()> {
        self.graph.remove(node)?;
        self.host.kill(node);
        Ok(())
    }

    /// 노드 프로세스를 띄운다. 끝나면 그래프에 알린다. Codex 노드면 메일함을 그 app-server로 넘기기 시작한다.
    fn launch(self: &Arc<Self>, id: NodeId, prepared: Prepared) -> Result<(), String> {
        let Prepared {
            cmd,
            setup,
            token,
            profile,
            codex,
            opencode2,
            account: _,
            inside: _,
        } = prepared;
        let on_exit = self.on_exit(id, &codex, &opencode2);
        self.host
            .spawn(id, cmd, setup, self.on_title(id, profile), on_exit)?;
        self.follow_screen(id, profile, &token);
        self.follow_servers(id, codex, opencode2, &token);
        Ok(())
    }

    /// 앞 데몬이 띄운 `running` 노드의 PTY 호스트와 서버에 다시 붙는다. 데몬이 시작할 때 한 번 부른다.
    /// 호스트가 없으면 노드는 코드 없는 `exited`가 되고, 남은 서버를 끝낸다.
    /// 그 뒤 붙지 않은 호스트(꺼진 동안 지운 노드, 앞 데몬의 로그인 창)를 끝낸다.
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
            self.follow_screen(node.id, profile, &node.token);
            self.follow_servers(node.id, codex, opencode2, &node.token);
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

    /// 노드 프로세스가 끝나면 그 서버를 끝내고 그래프에 알린다. 설정 스크립트가 실패했으면 그 출력의 끝을 함께 넘긴다.
    /// 서버는 맵이 아니라 이 클로저가 쥔다. 다시 붙은 호스트는 붙자마자 종료 코드를 보낼 수 있다.
    fn on_exit(
        self: &Arc<Self>,
        id: NodeId,
        codex: &Option<Arc<CodexServer>>,
        opencode2: &Option<Arc<opencode2::Node>>,
    ) -> impl FnOnce(Exit) + use<> {
        let app = self.clone();
        let discard = discarder(codex, opencode2);
        move |exit| {
            discard();
            // 다시 띄우기가 끝낸 프로세스다. 노드는 새 프로세스로 산다.
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
            // DB가 실패하면 알릴 곳이 없다. 화면은 running으로 남는다.
            let _ = match exit {
                Exit::Command(code) => app.graph.exited(id, code),
                Exit::Setup(code) => app.graph.setup_failed(id, code, &app.host.tail(id)),
            };
            // 종료를 적은 뒤 끝낸다. 남은 훅 프로세스가 다시 열면 `agent_idle`이 거절한다.
            app.idle.end(id);
        }
    }

    /// 노드 서버의 상태를 따라가고 메일함을 넘기기 시작한다.
    /// 노드 출력이 멎을 때마다, 그리고 세션이 시작할 때 지금 터미널 화면에서 확인 창을 찾아 사람 응답 대기로 옮긴다.
    /// 화면 상태를 보므로 다시 시작한 데몬도 같은 답을 낸다. 근거: docs/design.md 「사람 응답 대기」.
    /// `token`은 이 프로세스의 토큰이다. 같은 토큰의 호출만 세션을 시작한다(`Sessions::started`).
    fn follow_screen(self: &Arc<Self>, id: NodeId, profile: Option<Profile>, token: &str) {
        let dialogs = profile::dialogs(profile);
        let Some(attached) = self.host.attach(id) else {
            return;
        };
        if dialogs.is_empty() {
            return;
        }
        let mut started = self.sessions.launched(id, token);
        let app = self.clone();
        tokio::spawn(async move {
            let mut output = attached.output;
            // backlog도 화면이다. 붙은 직후 한 번 본다.
            let mut changed = true;
            loop {
                if !changed {
                    tokio::select! {
                        received = output.recv() => match received {
                            Ok(_) | Err(RecvError::Lagged(_)) => {}
                            Err(RecvError::Closed) => return,
                        },
                        // 새 프로세스가 자리를 받았다. 이 프로세스의 화면은 끝났다.
                        sent = started.changed() => if sent.is_err() {
                            return;
                        },
                    }
                }
                // 출력이 멎을 때까지 기다린다. 그리는 중인 창을 반쯤 읽지 않는다.
                loop {
                    match tokio::time::timeout(SCREEN_QUIET, output.recv()).await {
                        Ok(Ok(_) | Err(RecvError::Lagged(_))) => {}
                        Ok(Err(RecvError::Closed)) => return,
                        Err(_) => break,
                    }
                }
                changed = false;
                let found =
                    screen::find(dialogs, &app.host.screen(id), *started.borrow_and_update());
                let wait = found.as_ref().map(|(kind, text)| (*kind, text.as_str()));
                if let Err(e) = app.graph.set_wait(id, WaitSource::Screen, wait) {
                    eprintln!("loom: 노드 {}의 응답 대기를 쓰지 못했다: {e}", id.0);
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
            tokio::spawn(follow_codex_model(self.graph.clone(), id, server.clone()));
            // Codex는 훅이 없다. 폴더 신뢰·로그인 창이 닫혀야 스레드가 생긴다.
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
        let token = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        let unauthorized = || {
            (
                StatusCode::UNAUTHORIZED,
                "Run this only inside a loom node.".into(),
            )
        };
        let token = token.ok_or_else(unauthorized)?;
        let node = self
            .graph
            .caller(token)
            .map_err(|e| for_agent(Refusal::from(e)))?
            .ok_or_else(unauthorized)?;
        // 노드의 런타임이 세션을 시작했다. 첫 확인 창(폴더 신뢰, 로그인)은 세션 전에만 뜬다.
        self.sessions.started(node, token);
        Ok(node)
    }

    /// Claude·Codex면 설정에서 고른 계정의 로그인 폴더를 넣는다. Codex면 그 계정으로 app-server를 먼저 띄운다.
    /// OpenCode 2면 노드 서버와 세션을 먼저 띄운다.
    /// `cwd`는 노드가 도는 워크트리다. `position`은 상시 지시를 고른다(`skills::always_on`).
    async fn prepare(
        &self,
        start: &Start,
        cwd: &std::path::Path,
        position: Position,
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
        // WSL 폴더면 노드는 그 배포판 안에서 돌고 그 배포판의 계정을 쓴다. 근거: docs/design.md 「데몬 하나」.
        let site = match wsl::of(cwd) {
            Some(at) => Site::wsl(
                wsl::inside(&at, &profile::commands(start), &self.shim, &[&plugin])
                    .await
                    .map_err(LaunchError::Wsl)?,
            ),
            None => self.pc_site(),
        };
        let account = provider.and_then(|p| settings.account(p, site.distro()));
        if let Some(provider) = provider {
            env.extend(site.env(provider, account));
        }
        let inside = site.inside();
        // 에이전트가 읽는 스킬 플러그인 폴더와 그 안의 `skills/`. WSL 노드면 배포판 안에서 본 경로다.
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
                // 실패해도 노드는 띄운다. 다른 Codex 버전에는 이 요청이 없을 수 있다. 그때 노드는 내장 스킬만 없다.
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
            profile,
            codex,
            opencode2,
        })
    }

    /// 노드가 WSL 폴더에서 돌면 그 배포판 안의 위치다.
    fn node_wsl(&self, node: Option<NodeId>) -> Result<Option<wsl::WslPath>, Problem> {
        let Some(node) = node else { return Ok(None) };
        let place = self.graph.place_of(node).map_err(internal)?;
        Ok(wsl::of(std::path::Path::new(&place.worktree)))
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
    /// WSL 노드면 그 배포판에서 읽은 값. 설정 스크립트도 그 배포판에서 돈다.
    inside: Option<wsl::Inside>,
    /// `cmd`보다 먼저(같은 터미널 화면에서) 돌릴 저장소 설정 스크립트. 실패하면 `cmd`를 띄우지 않는다.
    setup: Option<portable_pty::CommandBuilder>,
    /// 노드가 데몬을 부를 때 내는 토큰. 그래프가 노드와 함께 남긴다.
    token: String,
    /// 노드가 쓰는 Claude·Codex 계정. 그래프가 노드와 함께 남긴다.
    account: Option<AccountId>,
    /// 터미널 제목에서 카드 주제를 꺼내는 프로필. 없으면 제목을 쓰지 않는다.
    profile: Option<Profile>,
    codex: Option<Arc<CodexServer>>,
    opencode2: Option<Arc<opencode2::Node>>,
}

impl Prepared {
    /// 그래프가 노드와 함께 남기는 값(토큰, 서버 주소, 계정). 프로세스를 띄우기 전에 남겨 노드가 뜨자마자 데몬을 불러도 찾게 한다.
    fn launched(&self) -> Launched {
        Launched {
            token: self.token.clone(),
            server: match (&self.codex, &self.opencode2) {
                (Some(server), _) => Some(server.link()),
                (_, Some(node)) => Some(node.link()),
                _ => None,
            },
            account: self.account.clone(),
        }
    }

    /// 노드와 같은 곳(이 PC 또는 그 WSL 배포판)에서 명령 한 줄을 도는 셸.
    fn shell(&self, line: &str) -> portable_pty::CommandBuilder {
        match &self.inside {
            Some(inside) => inside.wrap(&portable_pty::CommandBuilder::from_argv(wsl::shell(line))),
            None => profile::shell(line),
        }
    }

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
    app: Arc<App>,
    node: NodeId,
}

impl Mailbox for NodeMailbox {
    fn changed(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        let mut changed = Box::pin(self.app.graph.mail_changed());
        changed.as_mut().enable();
        // 확인 없이 둔 메시지는 새 메시지 알림 없이 다시 꺼낼 수 있게 된다.
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
        self.app.human_input(self.node).ok().flatten()
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

/// Codex 노드 스레드가 생기거나 상태가 바뀔 때마다 사람 응답 대기를 옮기고 모델을 다시 읽어 그래프에 옮긴다. 서버가 끝나면 끝난다.
/// 스레드가 새로 생기면 다시 붙는 값(`ServerLink`)도 그래프에 남긴다. 다시 시작한 데몬은 `thread/started`를 다시 받지 못한다.
async fn follow_codex_model(graph: Arc<Graph>, node: NodeId, server: Arc<CodexServer>) {
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
        if let Err(e) = graph.set_wait(node, WaitSource::Codex, waiting.map(|kind| (kind, ""))) {
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

/// Codex 노드의 메일함을 app-server로 넘긴다. 노드마다 이 작업 하나만 꺼내므로 같은 메시지를 두 번 넘기지 않는다.
/// 받는 노드의 상태에 따라 꺼내는 방식을 고른다. 근거: docs/design.md 「전달 지점」.
async fn deliver_to_codex(app: Arc<App>, node: NodeId, server: Arc<CodexServer>) {
    let mut thread = server.thread();
    // `turn/start`를 보낸 뒤 서버의 active 알림이 오기 전까지 일하는 중으로 본다. 그래야 큐를 두 턴으로 나눠 보내지 않는다.
    let mut started = false;
    let mut busy = false;
    // 이 작업이 `turn/start`로 연 턴이 아직 시작되지 않았다. 아니면 새 턴은 사람이 TUI에 입력한 턴이다.
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
            // 턴이 끝났다(일하는 중 → 쉬는 중). 다시 일하게 하면 이번에는 메시지를 넘기지 않는다.
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

impl App {
    /// 사람이 입력해 턴이 시작했다. 노드를 다시 일하는 상태로 두고, 그 입력에 붙일 안내를 돌려준다:
    /// 열린 `loom ask` 질문과, 부모 없는 노드면 완료 보고 안내(`profile::DONE_NOTE`).
    fn human_input(&self, node: NodeId) -> Result<Option<String>, Refusal> {
        self.graph.human_input(node)?;
        self.turn_ends.human(node);
        let questions = profile::questions_note(&self.graph.open_questions(node)?);
        let done = self
            .graph
            .relations(node)?
            .parent
            .is_none()
            .then(|| profile::DONE_NOTE.to_owned());
        let parts: Vec<String> = questions.into_iter().chain(done).collect();
        Ok((!parts.is_empty()).then(|| parts.join("\n\n")))
    }
}

/// 사람이 TUI에 입력해 턴이 시작했다. 열린 `loom ask` 질문이 있으면 그 턴에 한 줄을 더한다.
/// Codex에는 입력 훅이 없어 턴 안에 넣는다(`turn/steer`).
async fn codex_human_input(app: &App, node: NodeId, server: &CodexServer, thread: &str) {
    let text = match app.human_input(node) {
        Ok(Some(text)) => text,
        Ok(None) => return,
        Err(e) => return eprintln!("loom: 노드 {}의 사람 입력을 처리하지 못했다: {e}", node.0),
    };
    let call = |method, params| server.call(method, params);
    if let Err(e) = codex::steer(call, thread, &text).await {
        eprintln!("loom: 노드 {}에 열린 질문을 알리지 못했다: {e}", node.0);
    }
}

/// Codex 턴 끝 질문 검사. 다시 일하게 했으면 참이다. 대기 없는 루트만 답 글을 읽는다.
async fn codex_turn_end(app: &App, node: NodeId, server: &CodexServer, thread: &str) -> bool {
    let call = |method, params| server.call(method, params);
    if app.graph.unreported_root(node).unwrap_or(false) {
        match codex::last_reply(call, thread).await {
            Ok(Some(text)) => app.turn_ends.said(node, text),
            Ok(None) => {}
            Err(e) => eprintln!("loom: 노드 {}의 답 글을 읽지 못했다: {e}", node.0),
        }
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

/// 터미널에 붙여넣거나 끌어다 놓은 파일을 남기고 그 절대 경로를 돌려준다. 몸통은 파일 그대로, 경로 끝은 원래 파일 이름이다.
/// `node`가 WSL 노드면 그 배포판의 `/tmp`에 쓰고 배포판 안의 경로를 돌려준다. 그 노드의 CLI는 Windows 임시 폴더 경로를 읽지 못한다.
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
    let limits = app.tree_limits()?;
    app.graph.adopt(NodeId(parent), NodeId(id), limits)?;
    Ok(Json(()))
}

/// 사람이 터미널에서 누른 파일 링크를 기본 앱으로 연다. JSON 몸통이라 다른 출처의 페이지는 사전 요청에서 막힌다.
/// WSL 노드의 링크는 배포판 안의 경로(`/home/…`)다. 그 배포판의 UNC 경로로 연다.
async fn open_file(
    State(app): State<Arc<App>>,
    Json(req): Json<OpenFile>,
) -> Result<Json<()>, Problem> {
    let path = match app.node_wsl(req.node)? {
        Some(wsl) if req.path.starts_with('/') => wsl.unc(&req.path),
        _ => req.path,
    };
    open::open(std::path::Path::new(&path)).map_err(internal)?;
    Ok(Json(()))
}

/// 응답을 보낸 뒤 데몬이 끝나기까지의 여유. 응답 한 줄은 루프백에서 1ms 안에 간다.
const SHUTDOWN_AFTER: Duration = Duration::from_millis(200);

/// 사람이 `loom stop`이나 화면으로 데몬만 끝낸다. 노드의 PTY 호스트와 서버는 따로 도는 프로세스라 산다(`process_tree::detach`).
/// 다음 데몬이 노드에 다시 붙는다. 새 판의 `loom start`도 옛 판 데몬을 이 길로 끝낸다(`launch::ensure`).
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

/// 설정 › 정보의 [업데이트 확인]. 확인한 뒤의 상태를 돌려준다.
async fn check_update(State(app): State<Arc<App>>) -> Result<Json<UpdateView>, Problem> {
    app.update.clone().check().await.map(Json).map_err(internal)
}

/// [업데이트]. 받기·바꾸기·다시 뜨기는 뒤에서 하고, 단계가 바뀔 때마다 `ViewChanged`로 알린다.
async fn install_update(State(app): State<Arc<App>>) -> Result<Json<UpdateView>, Problem> {
    app.update.clone().install().map(Json).map_err(internal)
}

/// 사람이 화면에서 노드 이름을 바꾼다. 에이전트는 이 길을 쓰지 않는다(노드 문에 없다).
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

/// 받은편지함 줄의 [×]: 그 번호의 대기 하나(질문이든 신호의 대기든)를 지운다. 근거: docs/design.md 「사람 응답 대기」.
async fn dismiss_wait(
    State(app): State<Arc<App>>,
    Path(wait): Path<u32>,
) -> Result<Json<()>, Problem> {
    app.graph.dismiss(WaitId(wait))?;
    Ok(Json(()))
}

/// 사람이 화면에서 `loom ask` 질문에 답했다. 답은 사람이 보낸 메시지로 그 노드에 간다.
async fn answer_wait(
    State(app): State<Arc<App>>,
    Path(wait): Path<u32>,
    Json(req): Json<AnswerWait>,
) -> Result<Json<()>, Problem> {
    let node = app.graph.answer(WaitId(wait), &req.text, &req.images)?;
    app.turn_ends.human(node);
    Ok(Json(()))
}

/// 사람이 질문 줄의 문서 링크를 눌렀다. 데몬이 그 기계의 기본 앱(브라우저)으로 연다. 화면이 브라우저든 데스크톱 앱이든 같다.
/// 열린 질문에 붙은 문서만 연다. 파일을 매번 새로 열므로 새 판이 보인다.
async fn open_wait_doc(
    State(app): State<Arc<App>>,
    Path(wait): Path<u32>,
) -> Result<Json<()>, Problem> {
    let path = app.graph.wait_doc(WaitId(wait))?;
    open::open(&path).map_err(internal)?;
    Ok(Json(()))
}

/// 질문에 붙은 문서를 낸다. 폰과 브라우저 화면은 이 주소를 새 탭에서 연다. 앱은 `open_wait_doc`으로 기본 브라우저를 연다.
/// 주소가 `/`로 끝나서 문서 속 상대 주소(옆 그림)는 `wait_doc_file`로 온다.
async fn wait_doc(
    State(app): State<Arc<App>>,
    Path(wait): Path<u32>,
    request: axum::extract::Request,
) -> Result<Response, Problem> {
    let path = app.graph.wait_doc(WaitId(wait))?;
    serve_file(&path, request).await
}

/// 문서와 같은 폴더의 파일(목업의 그림). 폴더 밖(`..`, 절대 경로)은 내지 않는다.
async fn wait_doc_file(
    State(app): State<Arc<App>>,
    Path((wait, file)): Path<(u32, String)>,
    request: axum::extract::Request,
) -> Result<Response, Problem> {
    let doc = app.graph.wait_doc(WaitId(wait))?;
    let inside = std::path::Path::new(&file)
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)));
    match doc.parent() {
        Some(dir) if inside => serve_file(&dir.join(&file), request).await,
        _ => Ok(StatusCode::NOT_FOUND.into_response()),
    }
}

/// 완료 줄의 그림(`loom done --image`). 확인하지 않은 완료의 그림만 낸다.
async fn done_image_file(
    State(app): State<Arc<App>>,
    Path((wait, index)): Path<(u32, usize)>,
    request: axum::extract::Request,
) -> Result<Response, Problem> {
    let path = app.graph.done_image(WaitId(wait), index)?;
    serve_file(&path, request).await
}

async fn serve_file(
    path: &std::path::Path,
    request: axum::extract::Request,
) -> Result<Response, Problem> {
    let response = tower_http::services::ServeFile::new(path)
        .try_call(request)
        .await
        .map_err(internal)?;
    Ok(response.into_response())
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
                choice: req.choice,
                resume: None,
            },
            String::new(),
        ),
        SpawnRun::Command(line) => (Start::Command { line }, req.brief),
    };
    let place = app
        .graph
        .place_of(parent)
        .map_err(|e| for_agent(Refusal::from(e)))?;
    let worktree = match &req.worktree {
        Some(branch) => {
            let repo = app.repos.path(place.repo).map_err(for_agent)?;
            let workspace = app.workspace(&repo).await.map_err(for_agent)?;
            let from = std::path::Path::new(&place.worktree);
            let made = worktree::create(&repo, from, &workspace, branch, "HEAD")
                .await
                .map_err(for_agent)?;
            Some((repo, made))
        }
        None => None,
    };
    let made = worktree.as_ref().map(|(_, made)| made.as_str());
    let cwd = std::path::Path::new(made.unwrap_or(&place.worktree));
    let spawned = spawn_child(&app, parent, &req.name, &start, &queued_brief, made, cwd).await;
    // 자식을 띄우지 못하면 이 요청이 만든 워크트리와 브랜치를 지운다. 거절만 남긴다.
    if let (Some((repo, made)), Err(_)) = (&worktree, &spawned)
        && let Err(e) = worktree::discard(repo, made).await
    {
        eprintln!("loom: 띄우지 못한 자식의 워크트리 {made}를 지우지 못했다: {e}");
    }
    spawned.map(Json)
}

/// `agent_spawn`에서 워크트리를 만든 뒤의 단계: 준비하고 그래프에 자식을 더해 띄운다.
/// 새 워크트리면 그 설정 스크립트를 자식의 PTY에서 먼저 돌린다. 근거: docs/design.md 「저장소와 워크트리」.
async fn spawn_child(
    app: &Arc<App>,
    parent: NodeId,
    name: &str,
    start: &Start,
    queued_brief: &str,
    worktree: Option<&str>,
    cwd: &std::path::Path,
) -> Result<NodeId, AgentError> {
    let mut prepared = app
        .prepare(start, cwd, Position::Child)
        .await
        .map_err(for_agent)?;
    if worktree.is_some() {
        prepared.setup = worktree::setup_script(cwd).map(|line| prepared.shell(&line));
    }
    let discard = prepared.discarder();
    let limits = app
        .tree_limits()
        .map_err(for_agent)
        .inspect_err(|_| discard())?;
    app.graph
        .add_child(
            parent,
            NewChild {
                name,
                worktree,
                agent: start.agent(),
                brief: queued_brief,
            },
            limits,
            &prepared.launched(),
            |id| app.launch(id, prepared),
        )
        .map_err(for_agent)
        .inspect_err(|_| discard())
}

async fn agent_send(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<SendRequest>,
) -> Result<Json<Sent>, AgentError> {
    let from = app.caller(&headers)?;
    let sent = send(&app, from, &req);
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

fn send(app: &App, from: NodeId, req: &SendRequest) -> Result<Sent, Refusal> {
    if req.kind != Kind::Queue {
        let (to, agent) = app.graph.related(from, &req.to)?;
        let target = format!("#{}", to.0);
        let Some(profile) = agent.as_ref().and_then(profile::profile_of) else {
            return Err(Refusal::NotSteerable(req.to.clone()));
        };
        // 셸 앞에 `loom guard`를 붙이는 프로필만 그 셸을 끝낸다.
        if req.kind == Kind::Interrupt && matches!(profile, Profile::Claude | Profile::Cursor) {
            return app.graph.interrupt(from, &target, &req.body, |to| {
                app.host.stop_guarded(to);
            });
        }
        return app.graph.send(from, &target, req.kind, &req.body);
    }
    app.graph.send(from, &req.to, req.kind, &req.body)
}

async fn agent_reply(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<ReplyRequest>,
) -> Result<Json<Sent>, AgentError> {
    let from = app.caller(&headers)?;
    app.graph
        .reply(from, req.message, &req.body)
        .map(Json)
        .map_err(for_agent)
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

async fn agent_take(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<TakeRequest>,
) -> Result<Json<Taken>, AgentError> {
    let node = app.caller(&headers)?;
    // 훅이 불렸으면 턴이 돈다. 일하는 중의 메시지는 그 훅들이 넘긴다.
    app.idle.turn_started(node);
    keep_transcript(&app, node, req.transcript.as_deref())?;
    let messages = app
        .graph
        .take(node, &req.kinds, req.limit_bytes)
        .map_err(for_agent)?;
    let note = match req.human_input {
        true => app.human_input(node).map_err(for_agent)?,
        false => None,
    };
    Ok(Json(Taken { messages, note }))
}

/// 쉬는 Claude 노드의 비동기 훅이 부른다. 메시지가 오면 모두 꺼낸다(본문 합 상한은 훅과 같다).
/// 같은 노드의 새 대기나 다른 훅이 오면 빈 목록으로 끝난다. 그 훅은 아무것도 넘기지 않고 끝난다.
async fn agent_idle(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<Option<IdleRequest>>,
) -> Result<Json<Vec<Delivery>>, AgentError> {
    let node = app.caller(&headers)?;
    let req = req.unwrap_or_default();
    keep_transcript(&app, node, req.transcript.as_deref())?;
    // 프로세스 종료를 보기 전에 연다. 그 사이에 끝나면 `on_exit`가 이 대기를 끝낸다.
    let end = app.idle.open(node);
    if !app.graph.relations(node).map_err(for_agent)?.me.alive {
        app.idle.end(node);
        return Err((StatusCode::GONE, "The node process has exited.".to_owned()));
    }
    if req.rate_limited {
        app.rate_limited(node, profile::Provider::Claude);
    }
    app.turn_ended(node);
    if app.relaunches.holds_mail(node) {
        // 다시 띄우기가 이 대기를 끝낸다(`idle.end`). 메시지는 새 프로세스가 받는다.
        end.notified().await;
        return Ok(Json(Vec::new()));
    }
    tokio::select! {
        taken = app.graph.next(node, Kind::ALL, claude::HOOK_LIMIT_BYTES) => Ok(Json(taken.map_err(for_agent)?)),
        () = end.notified() => Ok(Json(Vec::new())),
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

/// 훅 입력의 세션 기록 파일(Claude, Cursor)을 남긴다. 다시 띄우기가 그 이름으로 세션을 잇고, 계정 전환이 Claude 기록을 새 계정으로 복사한다. 근거: docs/design.md 「계정 전환」.
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

async fn agent_done(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<DoneRequest>,
) -> Result<Json<Vec<String>>, AgentError> {
    let node = app.caller(&headers)?;
    for image in &req.images {
        done_image(image).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    }
    let alive_children = app
        .graph
        .done(node, &req.summary, req.asked.as_deref(), &req.images)
        .map_err(for_agent)?;
    Ok(Json(alive_children))
}

/// 브라우저가 `<img>`로 그리는 그림 꼴. 받은편지함이 완료 줄의 파일을 그대로 낸다.
const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];

/// 완료 줄의 그림은 받은편지함이 그대로 낸다. 있는 그림 파일만 받는다.
fn done_image(path: &std::path::Path) -> Result<(), String> {
    let image = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()));
    if !image {
        return Err(format!(
            "{}: --image takes an image file ({}).",
            path.display(),
            IMAGE_EXTENSIONS.join(", ")
        ));
    }
    if !path.is_file() {
        return Err(format!("{}: no such file.", path.display()));
    }
    Ok(())
}

/// 사람에게 묻는 일은 사람이 띄운 노드(부모 없음)만 한다. 다른 노드는 부모에게 묻는다.
async fn agent_ask(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<AskRequest>,
) -> Result<Json<Asked>, AgentError> {
    let node = app.caller(&headers)?;
    let attached = req
        .doc
        .as_deref()
        .map(user_doc::attach)
        .transpose()
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let doc = attached.as_ref().map(|a| a.doc.clone());
    let id = app
        .graph
        .ask(node, &req.question, attached)
        .map_err(for_agent)?;
    let open = app.graph.open_questions(node).map_err(for_agent)?;
    Ok(Json(Asked {
        id,
        doc,
        open: open.into_iter().map(|q| q.id).collect(),
    }))
}

/// `loom answered`: 사람의 입력이 `loom ask` 질문의 답이라고 에이전트가 판단했다. 그 질문만 지우고 지운 번호를 돌려준다. 다른 대기는 남는다.
/// 옛 CLI는 몸통 없이(`null`) 부른다. 번호 없는 호출과 같다.
async fn agent_answered(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<Option<AnsweredRequest>>,
) -> Result<Json<Vec<WaitId>>, AgentError> {
    let node = app.caller(&headers)?;
    let ids = req.unwrap_or_default().ids;
    Ok(Json(app.graph.answered(node, &ids).map_err(for_agent)?))
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

/// 훅이 부르는 턴 끝. 다시 일하게 할 글이 있으면 돌려준다. 훅은 그 글로 모델을 다시 깨우고 메시지를 꺼내지 않는다.
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
    let node = app.caller(&headers)?;
    app.graph
        .set_wait(
            node,
            WaitSource::Hook,
            req.wait.as_ref().map(|(kind, text)| (*kind, text.as_str())),
        )
        .map_err(|e| for_agent(Refusal::from(e)))?;
    Ok(Json(()))
}

async fn agent_adopt(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<AdoptRequest>,
) -> Result<Json<()>, AgentError> {
    let parent = app.caller(&headers)?;
    let child = app.graph.find(&req.name).map_err(for_agent)?;
    let limits = app.tree_limits().map_err(for_agent)?;
    app.graph.adopt(parent, child, limits).map_err(for_agent)?;
    Ok(Json(()))
}

/// 사람의 [노드 제거]와 같이 프로세스를 끝낸다. 지운 노드의 자식은 부르는 노드가 데려간다. 돌려준 값은 데려간 노드 이름이다.
async fn agent_remove(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<RemoveRequest>,
) -> Result<Json<Vec<String>>, AgentError> {
    let caller = app.caller(&headers)?;
    let limits = app.tree_limits().map_err(for_agent)?;
    let (removed, moved) = app
        .graph
        .remove_child(caller, &req.name, limits)
        .map_err(for_agent)?;
    app.host.kill(removed);
    Ok(Json(moved))
}

async fn pty(
    ws: WebSocketUpgrade,
    Path(id): Path<u32>,
    State(app): State<Arc<App>>,
    via: Option<Extension<ViaServe>>,
) -> Response {
    // 사람이 그 노드 터미널에 키를 누르면 답한 것으로 본다. `loom ask` 대기는 에이전트가 판단해 지운다. 근거: docs/design.md 「사람 응답 대기」.
    let typed = |app: &App, id| {
        if let Err(e) = app.graph.typed(id) {
            eprintln!("loom: 노드 {}의 응답 대기를 지우지 못했다: {e}", id.0);
        }
    };
    serve_pty(ws, via, app, |app| &app.host, typed, NodeId(id))
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
}

/// PTY 하나를 화면에 잇는다. 노드 터미널과 로그인 창이 함께 쓴다.
/// 사람이 누른 키(터미널이 스스로 보내는 응답은 빼고)가 오면 `typed`를 부른다.
/// 폰의 연결은 PC에서 그 기기를 끊으면 닫는다(`origin::until_removed`).
fn serve_pty<K: Copy + Eq + Hash + std::fmt::Display + Send + Sync + 'static>(
    ws: WebSocketUpgrade,
    via: Option<Extension<ViaServe>>,
    app: Arc<App>,
    host: fn(&App) -> &NodeHost<K>,
    typed: fn(&App, K),
    id: K,
) -> Response {
    let Some(attached) = host(&app).attach(id) else {
        // PTY가 없다. 화면이 안내를 보이게 알리고, 다시 붙지 않게 연결을 열어 둔다.
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
    ws.on_upgrade(move |socket| origin::until_removed(devices, via, async move {
        let (mut to_view, mut from_view) = socket.split();
        let mut output = attached.output;
        let mut size = attached.size;
        if send_size(&mut to_view, &mut size).await.is_err() {
            return;
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
                    // 밀린 화면은 끊는다. 화면이 다시 붙으면 backlog로 따라잡는다.
                    Err(RecvError::Lagged(_) | RecvError::Closed) => return,
                },
                changed = size.changed() => {
                    if changed.is_err() || send_size(&mut to_view, &mut size).await.is_err() {
                        return;
                    }
                }
                msg = from_view.next() => match msg {
                    Some(Ok(Message::Binary(bytes))) => {
                        host(&app).write(id, &bytes);
                        if node_host::typed(&bytes) {
                            typed(&app, id);
                        }
                    }
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
    }))
}

/// 지금 PTY 크기를 화면에 알리고 본 값으로 표시한다.
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
    via: Option<Extension<ViaServe>>,
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

/// 화면 빌드(vite)가 이름에 내용 해시를 붙여 두는 폴더.
const HASHED_ASSETS: &str = "assets/";

/// 첫 페이지(`/`)는 테마를 넣어 보내는 `settings::index`가 맡는다.
/// 해시 이름 파일은 내용이 바뀌면 이름도 바뀐다. 그래서 브라우저가 다시 묻지 않고 둔다. 이 머리가 없으면 새로 고칠 때마다 JS·글꼴 약 5MB를 다시 받았다(2026-10-22 측정).
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
            (StatusCode::BAD_REQUEST, "Give a name.".into())
        );
    }
}
