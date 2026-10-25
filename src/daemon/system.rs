use futures_util::stream::SplitSink;

use super::pulse::{self, Beat, Heard, Pulse};
use super::waits::serve_file;
use super::*;

pub(super) fn routes() -> Router<Arc<App>> {
    Router::new()
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

async fn open_file(
    State(app): State<Arc<App>>,
    Json(req): Json<OpenFile>,
) -> Result<Json<()>, Problem> {
    let path = host_path(&app, req.node, req.path)?;
    open::open(std::path::Path::new(&path)).map_err(internal)?;
    Ok(Json(()))
}

pub(super) fn host_path(app: &App, node: Option<NodeId>, path: String) -> Result<String, Problem> {
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

async fn event_log(State(app): State<Arc<App>>) -> Result<Json<EventLog>, Problem> {
    app.graph.event_log().map(Json).map_err(internal)
}

async fn events(
    ws: WebSocketUpgrade,
    State(app): State<Arc<App>>,
    via: Option<Extension<ViaRemote>>,
) -> Response {
    let devices = app.devices.clone();
    ws.on_upgrade(move |socket| {
        origin::until_removed(devices, via, send_events(socket, app, pulse::BEAT))
    })
}

async fn send_events(socket: WebSocket, app: Arc<App>, beat: Duration) {
    let (mut to_view, mut from_view) = socket.split();
    let mut pulse = Pulse::new(beat);
    loop {
        let Ok((snapshot, mut rx)) = app.graph.subscribe() else {
            return;
        };
        if send_json(&mut to_view, &GraphEvent::Snapshot { nodes: snapshot })
            .await
            .is_err()
        {
            return;
        }
        loop {
            tokio::select! {
                event = rx.recv() => match event {
                    Ok(event) => if send_json(&mut to_view, &event).await.is_err() { return },
                    Err(RecvError::Lagged(_)) => break,
                    Err(RecvError::Closed) => return,
                },
                heard = pulse.listen(&mut from_view) => match heard {
                    Heard::Frame(Some(Ok(_))) => {}
                    Heard::Frame(Some(Err(_)) | None) => return,
                    Heard::Beat(beat) => {
                        if beat == Beat::Dead
                            || to_view.send(Message::Ping(Default::default())).await.is_err()
                            || send_json(&mut to_view, &GraphEvent::Beat).await.is_err()
                        {
                            return;
                        }
                    }
                },
            }
        }
    }
}

async fn send_json(
    to_view: &mut SplitSink<WebSocket, Message>,
    value: &impl Serialize,
) -> Result<(), axum::Error> {
    let text = serde_json::to_string(value).expect("GraphEvent는 늘 직렬화된다");
    to_view.send(Message::Text(text.into())).await
}

const HASHED_ASSETS: &str = "assets/";

pub(super) async fn asset(uri: Uri) -> Response {
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
    use tokio_tungstenite::tungstenite::Message as Frame;

    const TEST_BEAT: Duration = Duration::from_millis(100);

    async fn events_socket() -> (
        tempfile::TempDir,
        Arc<App>,
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("loom.db");
        let app = Arc::new(
            App::new(
                crate::db::open(&db_path).unwrap(),
                &db_path,
                "127.0.0.1:0".parse().unwrap(),
                None,
                None,
                crate::tunnel::Door::new().0,
            )
            .unwrap(),
        );
        let routes = Router::new()
            .route(
                "/api/events",
                get(
                    |ws: WebSocketUpgrade, State(app): State<Arc<App>>| async move {
                        ws.on_upgrade(move |socket| send_events(socket, app, TEST_BEAT))
                    },
                ),
            )
            .with_state(app.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, routes).await });
        let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/api/events"))
            .await
            .unwrap();
        (dir, app, socket)
    }

    fn kind(frame: &Frame) -> String {
        let Frame::Text(text) = frame else {
            return format!("{frame:?}");
        };
        serde_json::from_str::<serde_json::Value>(text).unwrap()["kind"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[tokio::test]
    async fn the_events_socket_beats_while_the_view_answers() {
        let (_dir, _app, mut socket) = events_socket().await;
        let mut beats = 0;
        let until = tokio::time::Instant::now() + TEST_BEAT * 10;
        while let Ok(Some(Ok(frame))) = tokio::time::timeout_at(until, socket.next()).await {
            if kind(&frame) == "beat" {
                beats += 1;
            }
        }
        assert!(beats >= 5, "{beats} beats in 10 beat periods");
    }

    #[tokio::test]
    async fn the_events_socket_closes_when_the_view_stops_answering() {
        let (_dir, _app, mut socket) = events_socket().await;
        tokio::time::sleep(TEST_BEAT * 6).await;
        let mut frames = Vec::new();
        let ended = tokio::time::timeout(TEST_BEAT * 20, async {
            while let Some(Ok(frame)) = socket.next().await {
                if matches!(frame, Frame::Close(_)) {
                    break;
                }
                frames.push(kind(&frame));
            }
        })
        .await;
        assert!(ended.is_ok(), "the socket stayed open: {frames:?}");
        assert!(
            frames.iter().filter(|k| *k == "beat").count() <= 3,
            "{frames:?}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_view_that_falls_behind_gets_a_new_snapshot() {
        let (_dir, app, mut socket) = events_socket().await;
        let until = tokio::time::Instant::now() + Duration::from_secs(10);
        let mut next = async || loop {
            let frame = tokio::time::timeout_at(until, socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            if !matches!(frame, Frame::Ping(_)) && kind(&frame) != "beat" {
                return kind(&frame);
            }
        };
        assert_eq!(next().await, "snapshot");
        for _ in 0..crate::graph::EVENT_QUEUE * 2 {
            app.graph.publish_view_changed();
        }
        let mut kinds = Vec::new();
        while !kinds.contains(&"snapshot".to_owned()) {
            kinds.push(next().await);
        }
        assert!(
            kinds.iter().all(|k| k == "viewChanged" || k == "snapshot"),
            "{kinds:?}"
        );
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
}
