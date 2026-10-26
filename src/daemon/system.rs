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

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_BEAT: Duration = Duration::from_millis(100);

    fn test_beat(pace: Option<Pace>) -> Duration {
        match pace {
            None => TEST_BEAT,
            Some(Pace::Front) => TEST_BEAT / 4,
            Some(Pace::Back) => TEST_BEAT * 40,
        }
    }

    struct View {
        _dir: tempfile::TempDir,
        app: Arc<App>,
        to: tokio::sync::mpsc::UnboundedSender<Message>,
        from: tokio::sync::mpsc::UnboundedReceiver<Message>,
    }

    impl View {
        fn open() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let db_path = dir.path().join("loom.db");
            let app = Arc::new(
                App::new(
                    crate::db::open(&db_path).unwrap(),
                    &db_path,
                    "127.0.0.1:0".parse().unwrap(),
                    None,
                    None,
                    tokio::sync::mpsc::unbounded_channel().0,
                )
                .unwrap(),
            );
            let (to_view, from) = tokio::sync::mpsc::unbounded_channel::<Message>();
            let (to, from_view) = tokio::sync::mpsc::unbounded_channel();
            let to_view = Box::pin(futures_util::sink::unfold(
                to_view,
                async |to_view, message| to_view.send(message).map(|()| to_view),
            ));
            let from_view = Box::pin(futures_util::stream::unfold(
                from_view,
                async |mut from_view| {
                    let message = from_view.recv().await?;
                    Some((Ok::<_, std::convert::Infallible>(message), from_view))
                },
            ));
            tokio::spawn(send_events(to_view, from_view, app.clone(), test_beat));
            View {
                _dir: dir,
                app,
                to,
                from,
            }
        }

        fn send(&self, message: Message) {
            self.to.send(message).unwrap();
        }

        async fn next(&mut self, answers: bool) -> Option<Message> {
            let message = self.from.recv().await?;
            if answers && matches!(message, Message::Ping(_)) {
                self.send(Message::Pong(Default::default()));
            }
            Some(message)
        }

        async fn beats_for(&mut self, periods: u32) -> usize {
            let until = tokio::time::Instant::now() + TEST_BEAT * periods;
            let mut beats = 0;
            while let Ok(Some(message)) = tokio::time::timeout_at(until, self.next(true)).await {
                if kind(&message) == "beat" {
                    beats += 1;
                }
            }
            beats
        }
    }

    fn kind(message: &Message) -> String {
        let Message::Text(text) = message else {
            return format!("{message:?}");
        };
        serde_json::from_str::<serde_json::Value>(text.as_str()).unwrap()["kind"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[tokio::test(start_paused = true)]
    async fn the_events_socket_beats_while_the_view_answers() {
        let mut view = View::open();
        let beats = view.beats_for(10).await;
        assert!(beats >= 5, "{beats} beats in 10 beat periods");
    }

    #[tokio::test(start_paused = true)]
    async fn the_events_socket_beats_right_after_the_snapshot() {
        let mut view = View::open();
        let mut kinds = Vec::new();
        while kinds.len() < 2 {
            let message = view.next(true).await.unwrap();
            if !matches!(message, Message::Ping(_)) {
                kinds.push(kind(&message));
            }
        }
        assert_eq!(kinds, ["snapshot", "beat"]);
    }

    #[tokio::test(start_paused = true)]
    async fn the_events_socket_beats_at_the_pace_the_view_sends() {
        let mut view = View::open();
        view.send(Message::text(r#"{"pace":"back"}"#));
        let back = view.beats_for(20).await;
        assert!(back <= 2, "{back} beats at the back pace");
        view.send(Message::text(r#"{"pace":"front"}"#));
        let front = view.beats_for(20).await;
        assert!(front >= 5, "{front} beats at the front pace");
    }

    #[tokio::test(start_paused = true)]
    async fn the_events_socket_closes_when_the_view_stops_answering() {
        let mut view = View::open();
        let mut kinds = Vec::new();
        let ended = tokio::time::timeout(TEST_BEAT * 26, async {
            while let Some(message) = view.next(false).await {
                kinds.push(kind(&message));
            }
        })
        .await;
        assert!(ended.is_ok(), "the socket stayed open: {kinds:?}");
        assert!(
            kinds.iter().filter(|k| *k == "beat").count() <= 3,
            "{kinds:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_view_that_falls_behind_gets_a_new_snapshot() {
        let mut view = View::open();
        let next = async |view: &mut View| loop {
            let message = view.next(true).await.unwrap();
            if !matches!(message, Message::Ping(_)) && kind(&message) != "beat" {
                return kind(&message);
            }
        };
        assert_eq!(next(&mut view).await, "snapshot");
        for _ in 0..crate::graph::EVENT_QUEUE * 2 {
            view.app.graph.publish_view_changed();
        }
        let mut kinds = Vec::new();
        while !kinds.contains(&"snapshot".to_owned()) {
            kinds.push(next(&mut view).await);
        }
        assert!(
            kinds.iter().all(|k| k == "viewChanged" || k == "snapshot"),
            "{kinds:?}"
        );
    }

    #[test]
    fn the_pace_frame_is_one_small_object() {
        assert_eq!(
            serde_json::from_str::<Pace>(r#"{"pace":"front"}"#).unwrap(),
            Pace::Front
        );
        assert_eq!(beat(None), pulse::BEAT);
        assert_eq!(beat(Some(Pace::Front)), pulse::FRONT_BEAT);
        assert_eq!(beat(Some(Pace::Back)), pulse::BACK_BEAT);
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
