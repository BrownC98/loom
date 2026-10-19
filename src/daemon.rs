//! 조합 레이어: HTTP·WebSocket 라우트. 아래 기능 모듈을 부르고 결과를 엮기만 한다.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use rust_embed::Embed;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use ts_rs::TS;

use crate::node_host::NodeHost;
use crate::profile::{self, Start};
use crate::registry::{NodeId, NodeState, NodeView, Registry};

#[derive(Embed)]
#[folder = "web/dist"]
struct Assets;

struct App {
    repo: RepoView,
    host: NodeHost,
    registry: Arc<Registry>,
}

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

pub async fn serve(listen: SocketAddr) -> std::io::Result<()> {
    let path = std::env::current_dir()?;
    let app = Arc::new(App {
        repo: repo_view(path),
        host: NodeHost::default(),
        registry: Arc::new(Registry::new()),
    });
    let router = Router::new()
        .route("/api/repo", get(repo))
        .route("/api/nodes", post(create_node))
        .route("/api/nodes/{id}/pty", get(pty))
        .route("/api/events", get(events))
        .fallback(asset)
        .with_state(app);
    let listener = tokio::net::TcpListener::bind(listen).await?;
    println!("loom: http://{listen}");
    axum::serve(listener, router).await
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

async fn create_node(
    State(app): State<Arc<App>>,
    Json(req): Json<NewNode>,
) -> Result<Json<NodeId>, (StatusCode, String)> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "이름을 적는다.".into()));
    }
    let cmd = profile::command(&req.start, std::path::Path::new(&app.repo.path))
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let id = app.registry.reserve_id();
    let registry = app.registry.clone();
    app.host
        .spawn(id, cmd, move |code| {
            registry.set_state(id, NodeState::Exited { code })
        })
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("노드를 띄우지 못했다: {e}"),
            )
        })?;
    app.registry.insert(NodeView {
        id,
        name: name.to_owned(),
        profile: req.start.profile(),
        state: NodeState::Running,
    });
    Ok(Json(id))
}

async fn pty(ws: WebSocketUpgrade, Path(id): Path<u32>, State(app): State<Arc<App>>) -> Response {
    let id = NodeId(id);
    let Some(attached) = app.host.attach(id) else {
        return StatusCode::NOT_FOUND.into_response();
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
    let (snapshot, mut rx) = app.registry.subscribe();
    for view in snapshot {
        if send_json(&mut socket, &view).await.is_err() {
            return;
        }
    }
    // 밀리거나 끊기면 닫는다. 화면이 다시 붙으면 목록 전체를 다시 받는다.
    while let Ok(view) = rx.recv().await {
        if send_json(&mut socket, &view).await.is_err() {
            return;
        }
    }
}

async fn send_json(socket: &mut WebSocket, value: &impl Serialize) -> Result<(), axum::Error> {
    let text = serde_json::to_string(value).expect("NodeView는 늘 직렬화된다");
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
