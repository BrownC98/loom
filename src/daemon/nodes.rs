use super::pulse::{self, Beat, Heard, Pulse};
use super::*;

pub(super) fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/nodes", post(create_node))
        .route("/api/nodes/{id}", delete(remove_node))
        .route("/api/nodes/{id}/parent/{parent}", put(adopt_node))
        .route("/api/nodes/{id}/detach", post(detach_node))
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
        .route("/api/nodes/{id}/pty", get(pty))
        .route(
            "/api/pastes/{name}",
            post(save_paste).layer(DefaultBodyLimit::max(paste::LIMIT_BYTES)),
        )
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

#[derive(Deserialize)]
pub struct PasteAt {
    node: Option<NodeId>,
}

/// PTY 크기. 데몬은 바뀐 크기를 모든 화면에 알린다(`PtyNotice::Size`).
#[derive(Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Resize {
    pub cols: u16,
    pub rows: u16,
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
    let children = app.remove(NodeId(id)).map_err(internal)?;
    app.position_changed(&children);
    Ok(Json(()))
}

async fn adopt_node(
    State(app): State<Arc<App>>,
    Path((id, parent)): Path<(u32, u32)>,
) -> Result<Json<()>, Problem> {
    let limits = app.tree_limits()?;
    if app.graph.adopt(NodeId(parent), NodeId(id), limits)? {
        app.position_changed(&[NodeId(id)]);
    }
    app.parent_changed(&[NodeId(id)]);
    Ok(Json(()))
}

async fn detach_node(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
) -> Result<Json<()>, Problem> {
    app.graph.detach(NodeId(id))?;
    app.position_changed(&[NodeId(id)]);
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
    serve_pty(ws, via, app, |app| &app.host, human_typed, id)
}

pub(super) fn human_typed(app: &Arc<App>, id: NodeId, bytes: &[u8]) {
    let answered = app.graph.typed(id).unwrap_or_else(|e| {
        eprintln!("loom: 노드 {}의 응답 대기를 지우지 못했다: {e}", id.0);
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
        eprintln!("loom: 노드 {}의 입력 중 표시를 쓰지 못했다: {e}", id.0);
    }
}

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
            let mut pulse = Pulse::new(pulse::BEAT);
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
                        Heard::Frame(Some(Ok(Message::Binary(_)))) if !gone => app.sleeps.knocked(id),
                        Heard::Frame(Some(Ok(_))) => {}
                        Heard::Frame(Some(Err(_)) | None) => return,
                        Heard::Beat(beat) => {
                            if beat == Beat::Dead || socket.send(Message::Ping(Default::default())).await.is_err() {
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

async fn send_kept(socket: &mut WebSocket, kept: node_host::Kept) -> Result<(), axum::Error> {
    let node_host::PtySize { cols, rows } = kept.size;
    for notice in [PtyNotice::Size(Resize { cols, rows }), PtyNotice::Settled] {
        let notice = serde_json::to_string(&notice).expect("PtyNotice는 늘 직렬화된다");
        socket.send(Message::Text(notice.into())).await?;
    }
    socket.send(Message::Binary(kept.backlog)).await
}

/// 화면이 PTY 웹소켓에 보내는 글 프레임. 키 입력은 바이너리 프레임으로 간다(docs/design/데몬-하나.md 「데몬 하나」 PTY 열·줄).
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

pub(super) fn serve_pty<K: Copy + Eq + Hash + std::fmt::Display + Send + Sync + 'static>(
    ws: WebSocketUpgrade,
    via: Option<Extension<ViaRemote>>,
    app: Arc<App>,
    host: fn(&App) -> &NodeHost<K>,
    typed: fn(&Arc<App>, K, &[u8]),
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
    app: &Arc<App>,
    host: fn(&App) -> &NodeHost<K>,
    typed: fn(&Arc<App>, K, &[u8]),
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
    let mut pulse = Pulse::new(pulse::BEAT);
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
                    if beat == Beat::Dead || to_view.send(Message::Ping(Default::default())).await.is_err() {
                        return;
                    }
                }
                Heard::Frame(Some(Ok(Message::Binary(bytes)))) => {
                    let _ = host(app).write_if(id, |_| {
                        if node_host::typed(&bytes) {
                            typed(app, id, &bytes);
                        }
                        Ok::<_, std::convert::Infallible>(bytes.to_vec())
                    });
                }
                Heard::Frame(Some(Ok(Message::Text(text)))) => match serde_json::from_str(&text) {
                    Ok(PtyRequest::Resize(Resize { cols, rows })) => host(app).resize(id, cols, rows),
                    Ok(PtyRequest::Borrow(Resize { cols, rows })) => host(app).borrow(id, borrower, cols, rows),
                    Ok(PtyRequest::GiveBack) => host(app).give_back(id, borrower),
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

impl App {
    pub(super) fn position_changed(self: &Arc<Self>, nodes: &[NodeId]) {
        let active = self.graph.active().unwrap_or_default();
        for node in active
            .into_iter()
            .filter(|n| nodes.contains(&n.id) && relaunch::profile_of(n).is_some())
        {
            let planned = self.graph.plan_relaunch(node.id, |now| match now.pending {
                None if !now.ended => (
                    Some((crate::graph::Phase::AtTurnEnd, crate::graph::Plan::Resume)),
                    true,
                ),
                kept => (kept, false),
            });
            match planned {
                Ok(true) => eprintln!(
                    "loom: 노드 {}의 자리가 바뀌었다. 턴 끝에 다시 띄운다",
                    node.id.0
                ),
                Ok(false) => {}
                Err(e) => eprintln!(
                    "loom: 노드 {}의 다시 띄우기를 계획하지 못했다: {e}",
                    node.id.0
                ),
            }
            self.run_unless_busy(node.id);
        }
    }
}
