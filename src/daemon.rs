//! 조합 레이어: HTTP·WebSocket 라우트. 아래 기능 모듈을 부르고 결과를 엮기만 한다.

use std::collections::HashMap;
use std::ffi::OsString;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use rust_embed::Embed;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use ts_rs::TS;

use crate::db;
use crate::graph::{Delivery, Graph, GraphEvent, NodeId, Refusal};
use crate::node_host::NodeHost;
use crate::profile::{self, Profile, Start};

#[derive(Embed)]
#[folder = "web/dist"]
struct Assets;

struct App {
    repo: RepoView,
    host: NodeHost,
    graph: Arc<Graph>,
    /// 노드가 데몬을 부를 때 내는 토큰. 데몬이 다시 시작하면 노드 프로세스도 모두 끝나므로 메모리에만 둔다.
    tokens: Mutex<HashMap<String, NodeId>>,
    /// 노드 안에서 `loom` 명령이 쓰는 데몬 주소(`LOOM_URL`).
    url: String,
    /// 노드의 `PATH`. 앞에 이 실행 파일의 폴더를 넣어 노드가 `loom`을 바로 부르게 한다.
    path: OsString,
}

type ApiError = (StatusCode, String);

#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct RepoView {
    pub name: String,
    pub path: String,
}

#[derive(Deserialize, TS)]
#[ts(export)]
pub struct NewNode {
    pub name: String,
    pub start: Start,
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
    /// 셸이 해석하는 명령 한 줄.
    pub command: String,
    pub brief: String,
}

/// `loom send`의 요청.
#[derive(Serialize, Deserialize)]
pub struct SendRequest {
    pub to: String,
    pub body: String,
}

/// `loom done`의 요청. `summary`는 요약이나 결과 파일 경로다.
#[derive(Serialize, Deserialize)]
pub struct DoneRequest {
    pub summary: String,
}

pub async fn serve(listen: SocketAddr) -> std::io::Result<()> {
    let conn = db::open(&db::default_path()?).map_err(std::io::Error::other)?;
    let app = Arc::new(App {
        repo: repo_view(std::env::current_dir()?),
        host: NodeHost::default(),
        graph: Arc::new(Graph::open(conn).map_err(std::io::Error::other)?),
        tokens: Mutex::default(),
        url: node_url(listen),
        path: node_path()?,
    });
    let router = Router::new()
        .route("/api/repo", get(repo))
        .route("/api/nodes", post(create_node))
        .route("/api/agent/spawn", post(agent_spawn))
        .route("/api/agent/send", post(agent_send))
        .route("/api/agent/wait", post(agent_wait))
        .route("/api/agent/done", post(agent_done))
        .route("/api/nodes/{id}/pty", get(pty))
        .route("/api/events", get(events))
        .fallback(asset)
        .with_state(app);
    let listener = tokio::net::TcpListener::bind(listen).await?;
    println!("loom: http://{listen}");
    axum::serve(listener, router).await
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

fn repo_view(path: PathBuf) -> RepoView {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    RepoView {
        name,
        path: path.to_string_lossy().into_owned(),
    }
}

async fn repo(State(app): State<Arc<App>>) -> Json<RepoView> {
    Json(app.repo.clone())
}

impl App {
    /// 노드 프로세스를 띄운다. 끝나면 그래프에 알린다.
    fn launch(
        self: &Arc<Self>,
        id: NodeId,
        mut cmd: portable_pty::CommandBuilder,
    ) -> Result<(), String> {
        let token = new_token()?;
        cmd.env("LOOM_URL", &self.url);
        cmd.env("LOOM_TOKEN", &token);
        cmd.env("PATH", &self.path);
        // 노드가 뜨자마자 데몬을 불러도 받도록 토큰을 먼저 넣는다.
        self.tokens.lock().unwrap().insert(token.clone(), id);
        let graph = self.graph.clone();
        self.host
            .spawn(id, cmd, move |code| {
                // DB가 실패하면 알릴 곳이 없다. 화면은 running으로 남는다.
                let _ = graph.exited(id, code);
            })
            .inspect_err(|_| {
                self.tokens.lock().unwrap().remove(&token);
            })
    }

    fn caller(&self, headers: &HeaderMap) -> Result<NodeId, ApiError> {
        let token = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        token
            .and_then(|t| self.tokens.lock().unwrap().get(t).copied())
            .ok_or((
                StatusCode::UNAUTHORIZED,
                "loom 노드 안에서만 부른다.".into(),
            ))
    }

    fn command(&self, start: &Start) -> Result<portable_pty::CommandBuilder, ApiError> {
        profile::command(start, std::path::Path::new(&self.repo.path))
            .map_err(|e| (StatusCode::BAD_REQUEST, e))
    }
}

fn new_token() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn refused(e: Refusal) -> ApiError {
    let status = match e {
        Refusal::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_REQUEST,
    };
    (status, e.to_string())
}

async fn create_node(
    State(app): State<Arc<App>>,
    Json(req): Json<NewNode>,
) -> Result<Json<NodeId>, ApiError> {
    let cmd = app.command(&req.start)?;
    let id = app
        .graph
        .add_root(&req.name, req.start.profile(), |id| app.launch(id, cmd))
        .map_err(refused)?;
    Ok(Json(id))
}

async fn agent_spawn(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<SpawnRequest>,
) -> Result<Json<NodeId>, ApiError> {
    let parent = app.caller(&headers)?;
    let cmd = app.command(&Start::Command { line: req.command })?;
    let id = app
        .graph
        .add_child(parent, &req.name, Profile::Command, &req.brief, |id| {
            app.launch(id, cmd)
        })
        .map_err(refused)?;
    Ok(Json(id))
}

async fn agent_send(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<SendRequest>,
) -> Result<Json<()>, ApiError> {
    let from = app.caller(&headers)?;
    app.graph.send(from, &req.to, &req.body).map_err(refused)?;
    Ok(Json(()))
}

async fn agent_wait(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
) -> Result<Json<Delivery>, ApiError> {
    let node = app.caller(&headers)?;
    Ok(Json(app.graph.wait(node).await.map_err(refused)?))
}

async fn agent_done(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<DoneRequest>,
) -> Result<Json<()>, ApiError> {
    let node = app.caller(&headers)?;
    app.graph.done(node, &req.summary).map_err(refused)?;
    Ok(Json(()))
}

async fn pty(ws: WebSocketUpgrade, Path(id): Path<u32>, State(app): State<Arc<App>>) -> Response {
    let id = NodeId(id);
    let Some(attached) = app.host.attach(id) else {
        // 데몬이 다시 시작해 PTY가 없는 노드다. 안내 한 줄을 보이고, 화면이 다시 붙지 않게 연결을 열어 둔다.
        return ws.on_upgrade(|mut socket| async move {
            let notice =
                Bytes::from_static("데몬이 다시 시작해 이 노드의 터미널은 없다.\r\n".as_bytes());
            if socket.send(Message::Binary(notice)).await.is_ok() {
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
                    Some(Ok(Message::Binary(bytes))) => app.host.write(id, &bytes),
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(Resize { cols, rows }) = serde_json::from_str(&text) {
                            app.host.resize(id, cols, rows);
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => return,
                },
            }
        }
    })
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

async fn asset(uri: Uri) -> Response {
    let path = match uri.path().trim_start_matches('/') {
        "" => "index.html",
        p => p,
    };
    match Assets::get(path) {
        Some(file) => (
            [(header::CONTENT_TYPE, file.metadata.mimetype().to_owned())],
            file.data,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
