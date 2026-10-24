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
