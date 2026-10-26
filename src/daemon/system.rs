use futures_util::{Sink, Stream};

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

/// 화면이 `/api/events`로 보내는 글 프레임. 데몬은 이 값에 맞춰 박동 간격을 바꾼다.
/// 보내지 않은 화면에는 15초마다 박동을 보낸다.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, TS)]
#[serde(tag = "pace", rename_all = "camelCase")]
#[ts(export)]
pub enum Pace {
    /// 사람이 화면을 보고 있다.
    Front,
    /// 화면이 뒤에 있다. 휴대폰 서비스는 늘 이것을 보낸다.
    Back,
}

fn beat(pace: Option<Pace>) -> Duration {
    match pace {
        None => pulse::BEAT,
        Some(Pace::Front) => pulse::FRONT_BEAT,
        Some(Pace::Back) => pulse::BACK_BEAT,
    }
}

async fn events(
    ws: WebSocketUpgrade,
    State(app): State<Arc<App>>,
    via: Option<Extension<ViaRemote>>,
) -> Response {
    let devices = app.devices.clone();
    ws.on_upgrade(move |socket| {
        let (to_view, from_view) = socket.split();
        origin::until_removed(devices, via, send_events(to_view, from_view, app, beat))
    })
}

async fn send_events<E>(
    mut to_view: impl Sink<Message> + Unpin,
    mut from_view: impl Stream<Item = Result<Message, E>> + Unpin,
    app: Arc<App>,
    beat: fn(Option<Pace>) -> Duration,
) {
    let mut pulse = Pulse::new(beat(None));
    loop {
        let Ok((snapshot, mut rx)) = app.graph.subscribe() else {
            return;
        };
        if send_json(&mut to_view, &GraphEvent::Snapshot { nodes: snapshot })
            .await
            .is_err()
            || send_json(&mut to_view, &GraphEvent::Beat).await.is_err()
        {
            return;
        }
        pulse.sent();
        loop {
            tokio::select! {
                event = rx.recv() => match event {
                    Ok(event) => {
                        if send_json(&mut to_view, &event).await.is_err() {
                            return;
                        }
                        pulse.sent();
                    }
                    Err(RecvError::Lagged(_)) => break,
                    Err(RecvError::Closed) => return,
                },
                heard = pulse.listen(&mut from_view) => match heard {
                    Heard::Frame(Some(Ok(Message::Text(text)))) => {
                        if let Ok(pace) = serde_json::from_str(&text) {
                            pulse.pace(beat(Some(pace)));
                        }
                    }
                    Heard::Frame(Some(Ok(_))) => {}
                    Heard::Frame(Some(Err(_)) | None) => return,
                    Heard::Beat(Beat::Dead) => return,
                    Heard::Beat(Beat::Ping) => {
                        if to_view.send(Message::Ping(Default::default())).await.is_err() {
                            return;
                        }
                    }
                    Heard::Beat(Beat::Send) => {
                        if to_view.send(Message::Ping(Default::default())).await.is_err()
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

async fn send_json<S: Sink<Message> + Unpin>(
    to_view: &mut S,
    value: &impl Serialize,
) -> Result<(), S::Error> {
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
