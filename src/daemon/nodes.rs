use futures_util::Sink;

use api::graph::events::EventRecord;
use api::graph::pty::{PtyNotice, PtyRequest, Resize};
use api::graph::{NewNode, RenameNode};

use super::pulse::{Beat, Heard, Pulse};
use super::*;
use crate::distro::SSH_NOT_CONNECTED;
use crate::place::{self, Machine};

pub(super) fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/nodes", get(list_nodes).post(create_node))
        .route("/api/nodes/{id}", delete(remove_node))
        .route("/api/nodes/{id}/parent/{parent}", put(adopt_node))
        .route("/api/nodes/{id}/detach", post(detach_node))
        .route("/api/nodes/{id}/name", put(rename_node))
        .route("/api/nodes/{id}/steer-queued", post(steer_queued))
        .route("/api/nodes/{id}/wake", post(wake_node))
        .route(
            "/api/nodes/{id}/bookmark",
            put(bookmark_node).delete(unbookmark_node),
        )
        .route(
            "/api/nodes/{id}/card-view",
            put(set_card_view).delete(clear_card_view),
        )
        .route("/api/nodes/{id}/pty", get(pty))
        .route("/api/nodes/{id}/process-log", get(process_log))
        .route(
            "/api/pastes/{name}",
            post(save_paste).layer(DefaultBodyLimit::max(paste::LIMIT_BYTES)),
        )
}

async fn list_nodes(State(app): State<Arc<App>>) -> Result<Json<Vec<NodeView>>, Problem> {
    app.graph
        .views(|node| app.tools.of(node))
        .map(Json)
        .map_err(internal)
}

async fn process_log(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
) -> Result<Json<Vec<EventRecord>>, Problem> {
    app.graph
        .process_log(NodeId(id))
        .map(Json)
        .map_err(internal)
}

#[derive(Deserialize)]
pub struct PasteAt {
    node: Option<NodeId>,
}

async fn create_node(
    State(app): State<Arc<App>>,
    Json(req): Json<NewNode>,
) -> Result<Json<NodeId>, Problem> {
    let repo = app.repos.path(req.place.repo)?;
    worktree::find(app.files(), &repo, &req.place.worktree).await?;
    Ok(Json(app.start_root(&req.place, &req.start).await?))
}

async fn save_paste(
    State(app): State<Arc<App>>,
    Path(name): Path<String>,
    Query(at): Query<PasteAt>,
    body: Bytes,
) -> Result<Json<String>, Problem> {
    let machine = app.node_machine(at.node)?;
    let root = match &machine {
        Machine::Pc => std::env::temp_dir(),
        Machine::Wsl(wsl) => PathBuf::from(wsl.unc("/tmp")),
        Machine::Ssh(at) => return Err(internal(format!("{}: {SSH_NOT_CONNECTED}", at.host))),
    };
    let path = paste::save(&root, &name, &body).map_err(internal)?;
    Ok(Json(place::inner(&path.to_string_lossy())))
}

async fn remove_node(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
) -> Result<Json<()>, Problem> {
    app.remove(NodeId(id))?;
    Ok(Json(()))
}

async fn adopt_node(
    State(app): State<Arc<App>>,
    Path((id, parent)): Path<(u32, u32)>,
) -> Result<Json<()>, Problem> {
    let limits = app.tree_limits()?;
    app.graph.adopt(NodeId(parent), NodeId(id), limits)?;
    app.parent_changed(&[NodeId(id)]);
    Ok(Json(()))
}

async fn detach_node(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
) -> Result<Json<()>, Problem> {
    app.graph.detach(NodeId(id))?;
    app.parent_changed(&[NodeId(id)]);
    Ok(Json(()))
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

async fn pty(
    ws: WebSocketUpgrade,
    Path(id): Path<u32>,
    State(app): State<Arc<App>>,
    via: Option<Extension<ViaRemote>>,
) -> Response {
    let id = NodeId(id);
    if app.host.attach(id).is_none() {
        return coming_pty(ws, via, app, id);
    }
    let input = PtyInput {
        typed: live_typed,
        composing: live_composing,
    };
    serve_pty(ws, via, app, |app| &app.host, input, id)
}

async fn wake_node(State(app): State<Arc<App>>, Path(id): Path<u32>) -> Json<()> {
    app.sleeps.knocked(NodeId(id));
    Json(())
}

fn live_composing(app: &Arc<App>, id: NodeId, pty: &node_host::Keys<'_>) {
    if asleep(app, id) {
        app.sleeps.keep(id, &[], pty.serial());
    }
    app.sleeps.knocked(id);
}

fn asleep(app: &App, id: NodeId) -> bool {
    app.graph
        .process(id)
        .is_ok_and(|p| p == Some(crate::graph::ProcessOutcome::Asleep))
}

pub(super) struct PtyInput<K> {
    pub typed: fn(&Arc<App>, K, &node_host::Keys<'_>, &[u8]) -> Typed,
    pub composing: fn(&Arc<App>, K, &node_host::Keys<'_>),
}

pub(super) enum Typed {
    Write,
    Kept,
}

fn live_typed(app: &Arc<App>, id: NodeId, pty: &node_host::Keys<'_>, bytes: &[u8]) -> Typed {
    if !asleep(app, id) && !app.sleeps.holds(id) {
        human_typed(app, id, bytes);
        return Typed::Write;
    }
    if has_text(bytes) {
        app.sleeps.keep(id, bytes, pty.serial());
    }
    if asleep(app, id) {
        app.sleeps.knocked(id);
    }
    let app = app.clone();
    tokio::spawn(async move { app.give_kept_keys(id) });
    Typed::Kept
}

pub(super) fn human_typed(app: &Arc<App>, id: NodeId, bytes: &[u8]) {
    let answered = app.graph.typed(id).unwrap_or_else(|e| {
        crate::log::logln!("loom: 노드 {}의 응답 대기를 지우지 못했다: {e}", id.0);
        true
    });
    app.sleeps.knocked(id);
    if answered {
        return;
    }
    if CLEAR_INPUT.contains(&bytes) {
        return app.draft_ended(id);
    }
    if has_text(bytes)
        && let Err(e) = app.graph.begin_draft(id)
    {
        crate::log::logln!("loom: 노드 {}의 입력 중 표시를 쓰지 못했다: {e}", id.0);
    }
    if bytes.contains(&SUBMIT) && app.profile(id).is_some_and(profile::idle_holds_input) {
        app.idle.submit_input(id);
    }
}

const SUBMIT: u8 = b'\r';

const CLEAR_INPUT: [&[u8]; 2] = [b"\x03", b"\x1b"];

fn has_text(bytes: &[u8]) -> bool {
    let mut rest = bytes;
    while let Some((&first, tail)) = rest.split_first() {
        rest = match (first, tail) {
            (0x1b, [b'[', b'M', _, _, _, x10 @ ..]) => x10,
            (0x1b, [b'[', csi @ ..]) => {
                let end = csi.iter().position(|b| (0x40..=0x7e).contains(b));
                end.map_or(&[][..], |end| &csi[end + 1..])
            }
            (0x1b, [b'O', _, ss3 @ ..]) => ss3,
            (0x1b, [_, other @ ..]) => other,
            (control, _) if control < 0x20 || control == 0x7f => tail,
            _ => return true,
        };
    }
    false
}

fn coming_pty(
    ws: WebSocketUpgrade,
    via: Option<Extension<ViaRemote>>,
    app: Arc<App>,
    id: NodeId,
) -> Response {
    let devices = app.devices.clone();
    let kept = app.host.kept(id);
    ws.on_upgrade(move |mut socket| {
        origin::until_removed(devices, via, async move {
            if let Some(kept) = kept
                && send_kept(&mut socket, kept).await.is_err()
            {
                return;
            }
            let mut pulse = Pulse::new();
            if send_beat(&mut socket, &pulse).await.is_err() {
                return;
            }
            let mut nodes = app.graph.changes();
            let mut gone = false;
            loop {
                let changed = app.ptys_changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                let reachable = app.graph.reachable(id).unwrap_or(false);
                if app.host.attach(id).is_some() || (gone && reachable) {
                    let _ = socket.send(Message::Close(None)).await;
                    return;
                }
                if !gone && !reachable {
                    gone = true;
                    let notice =
                        serde_json::to_string(&PtyNotice::Gone).expect("PtyNotice는 늘 직렬화된다");
                    if socket.send(Message::Text(notice.into())).await.is_err() {
                        return;
                    }
                }
                tokio::select! {
                    heard = pulse.listen(&mut socket) => match heard {
                        Heard::Frame(Some(Ok(Message::Binary(bytes)))) if !gone => {
                            if has_text(&bytes) {
                                app.sleeps.keep(id, &bytes, None);
                            }
                            app.sleeps.knocked(id);
                            app.give_kept_keys(id);
                        }
                        Heard::Frame(Some(Ok(Message::Text(text)))) => {
                            if pulse.paced(&text) {
                                if send_beat(&mut socket, &pulse).await.is_err() {
                                    return;
                                }
                            } else if !gone && matches!(serde_json::from_str(&text), Ok(PtyRequest::Composing)) {
                                app.sleeps.keep(id, &[], None);
                                app.sleeps.knocked(id);
                            }
                        }
                        Heard::Frame(Some(Ok(_))) => {}
                        Heard::Frame(Some(Err(_)) | None) => return,
                        Heard::Beat(beat) => {
                            if beat == Beat::Dead
                                || socket.send(Message::Ping(Default::default())).await.is_err()
                                || (beat == Beat::Send && send_beat(&mut socket, &pulse).await.is_err())
                            {
                                return;
                            }
                        }
                    },
                    () = changed => {}
                    _ = nodes.recv() => {}
                }
            }
        })
    })
}

async fn send_beat<S: Sink<Message> + Unpin>(
    to_view: &mut S,
    pulse: &Pulse,
) -> Result<(), S::Error> {
    let beat = PtyNotice::Beat {
        quiet_ms: pulse.quiet_ms(),
    };
    let notice = serde_json::to_string(&beat).expect("PtyNotice는 늘 직렬화된다");
    to_view.send(Message::Text(notice.into())).await
}

async fn send_kept(socket: &mut WebSocket, kept: node_host::Kept) -> Result<(), axum::Error> {
    let node_host::PtySize { cols, rows } = kept.size;
    for notice in [PtyNotice::Size(Resize { cols, rows }), PtyNotice::Settled] {
        let notice = serde_json::to_string(&notice).expect("PtyNotice는 늘 직렬화된다");
        socket.send(Message::Text(notice.into())).await?;
    }
    socket.send(Message::Binary(kept.backlog)).await
}

pub(super) fn serve_pty<K: Copy + Eq + Hash + std::fmt::Display + Send + Sync + 'static>(
    ws: WebSocketUpgrade,
    via: Option<Extension<ViaRemote>>,
    app: Arc<App>,
    host: fn(&App) -> &NodeHost<K>,
    input: PtyInput<K>,
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
            relay_pty(socket, attached, &app, host, input, id, borrower).await;
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
    app: &Arc<App>,
    host: fn(&App) -> &NodeHost<K>,
    input: PtyInput<K>,
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
    let mut pulse = Pulse::new();
    if send_beat(&mut to_view, &pulse).await.is_err() {
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
            heard = pulse.listen(&mut from_view) => match heard {
                Heard::Beat(beat) => {
                    if beat == Beat::Dead
                        || to_view.send(Message::Ping(Default::default())).await.is_err()
                        || (beat == Beat::Send && send_beat(&mut to_view, &pulse).await.is_err())
                    {
                        return;
                    }
                }
                Heard::Frame(Some(Ok(Message::Binary(bytes)))) => {
                    let _ = host(app).write_if(id, |keys| {
                        if node_host::typed(&bytes) && matches!((input.typed)(app, id, keys, &bytes), Typed::Kept) {
                            return Ok(Vec::new());
                        }
                        Ok::<_, std::convert::Infallible>(bytes.to_vec())
                    });
                }
                Heard::Frame(Some(Ok(Message::Text(text)))) if pulse.paced(&text) => {
                    if send_beat(&mut to_view, &pulse).await.is_err() {
                        return;
                    }
                }
                Heard::Frame(Some(Ok(Message::Text(text)))) => match serde_json::from_str(&text) {
                    Ok(PtyRequest::Resize(Resize { cols, rows })) => host(app).resize(id, cols, rows),
                    Ok(PtyRequest::Borrow(Resize { cols, rows })) => host(app).borrow(id, borrower, cols, rows),
                    Ok(PtyRequest::GiveBack) => host(app).give_back(id, borrower),
                    Ok(PtyRequest::Composing) => {
                        let _ = host(app).strokes_if(id, std::time::Duration::ZERO, |keys| {
                            (input.composing)(app, id, keys);
                            Ok::<_, std::convert::Infallible>(Vec::new())
                        });
                    }
                    Err(_) => {}
                },
                Heard::Frame(Some(Ok(_))) => {}
                Heard::Frame(Some(Err(_)) | None) => return,
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
