use super::runtime::bearer;
use super::*;

pub(super) fn node_routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/agent/note", post(agent_note))
        .route("/api/agent/wait", post(agent_wait))
        .route("/api/agent/answer", post(agent_answer))
        .route("/api/agent/status", post(agent_status))
        .route("/api/agent/nodes", post(agent_nodes))
        .route("/api/agent/take", post(agent_take))
        .route("/api/agent/idle", post(agent_idle))
        .route("/api/agent/ack", post(agent_ack))
        .route(
            "/api/agent/interrupt-pending",
            post(agent_interrupt_pending),
        )
        .route("/api/agent/guard", post(agent_guard))
        .route("/api/agent/model", post(agent_model))
        .route("/api/agent/roles", post(agent_roles))
        .route("/api/agent/roles/set", post(agent_set_role))
        .route("/api/agent/question", post(agent_question))
        .route("/api/agent/done", post(agent_done))
        .route("/api/agent/ask", post(agent_ask))
        .route("/api/agent/hook-wait", post(agent_hook_wait))
        .route("/api/agent/answered", post(agent_answered))
        .route("/api/agent/said", post(agent_said))
        .route("/api/agent/question-box", post(agent_question_box))
        .route("/api/agent/turn-end", post(agent_turn_end))
        .route("/api/agent/adopt", post(agent_adopt))
        .route("/api/agent/rm", post(agent_remove))
        .route("/api/agent/relaunch", post(relaunch::agent_relaunch))
        .route("/api/agent/ready", post(gate::agent_ready))
        .route("/api/agent/verdict", post(gate::agent_verdict))
        .route("/api/agent/fail", post(gate::agent_fail))
        .route("/api/agent/task", post(agent_task))
}

pub(super) fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/agent/spawn", post(agent_spawn))
}

#[derive(Serialize, Deserialize)]
pub struct SpawnRequest {
    pub run: SpawnRun,
    pub brief: String,
    pub worktree: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SpawnRun {
    Role { name: String, role: String },
    Review { role: String, branch: String },
    Command { name: String, line: String },
}

impl SpawnRun {
    pub fn name(&self) -> String {
        match self {
            SpawnRun::Role { name, .. } | SpawnRun::Command { name, .. } => name.clone(),
            SpawnRun::Review { role, branch } => format!("{} {}", role.trim(), branch.trim()),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct NoteRequest {
    pub to: String,
    pub mode: Kind,
    pub body: String,
}

#[derive(Serialize, Deserialize)]
pub struct AnswerRequest {
    pub message: MessageId,
    pub body: String,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WaitRequest {
    pub reply_to: Option<MessageId>,
    pub timeout_secs: Option<u64>,
}

#[derive(Serialize, Deserialize)]
pub struct TakeRequest {
    #[serde(default)]
    pub hook: String,
    pub kinds: Vec<Kind>,
    pub limit_bytes: usize,
    #[serde(default)]
    pub transcript: Option<String>,
    #[serde(default)]
    pub human_input: bool,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub turn: Option<TurnId>,
}

#[derive(Serialize, Deserialize)]
pub struct Taken {
    pub messages: Vec<Delivery>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct IdleRequest {
    pub transcript: Option<String>,
    pub rate_limited: bool,
    pub turn: Option<TurnId>,
}

#[derive(Serialize, Deserialize)]
pub struct GuardRequest {
    pub caller: u32,
}

#[derive(Serialize, Deserialize)]
pub struct AckRequest {
    pub ids: Vec<MessageId>,
}

#[derive(Serialize, Deserialize)]
pub struct AdoptRequest {
    pub name: String,
}

#[derive(Serialize, Deserialize)]
pub struct RemoveRequest {
    pub name: String,
}

#[derive(Serialize, Deserialize)]
pub struct RelaunchRequest {
    pub name: Option<String>,
    #[serde(default)]
    pub to: Option<NewAgent>,
}

#[derive(Serialize, Deserialize)]
pub struct AskRequest {
    pub question: String,
    #[serde(default)]
    pub doc: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AskResult {
    Inbox(Asked),
    Parent(Sent),
}

#[derive(Serialize, Deserialize)]
pub struct Asked {
    pub id: WaitId,
    pub doc: Option<WaitDoc>,
    #[serde(default)]
    pub open: Vec<WaitId>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct AnsweredRequest {
    #[serde(default)]
    pub ids: Vec<WaitId>,
}

#[derive(Deserialize)]
struct QuestionRequest {
    id: WaitId,
}

#[derive(Serialize)]
struct QuestionState {
    answered: bool,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TurnId(pub String);

#[derive(Serialize, Deserialize)]
pub struct SaidRequest {
    pub text: String,
    #[serde(default)]
    pub turn: Option<TurnId>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TurnEndRequest {
    pub turn: Option<TurnId>,
}

#[derive(Serialize, Deserialize)]
pub struct HookWaitRequest {
    pub wait: Option<(WaitKind, String)>,
}

#[derive(Serialize, Deserialize)]
pub struct DoneRequest {
    pub summary: String,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub asked: Option<String>,
    #[serde(default)]
    pub images: Vec<PathBuf>,
    #[serde(default)]
    pub doc: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
pub struct Reported {
    pub alive: Vec<String>,
    pub warning: Option<String>,
}

async fn agent_task(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
) -> Result<Json<Option<String>>, AgentError> {
    let caller = app.caller(&headers)?;
    let dir = app.task_dir(caller).await.map_err(for_agent)?;
    Ok(Json(Some(dir.local.to_string_lossy().into_owned())))
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

async fn agent_roles(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
) -> Result<Json<Roles>, AgentError> {
    app.caller(&headers)?;
    let settings = app.settings.get().map_err(|e| for_agent(internal(e)))?;
    Ok(Json(settings.roles))
}

async fn agent_question(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<QuestionRequest>,
) -> Result<Json<QuestionState>, AgentError> {
    app.caller(&headers)?;
    let answered = app
        .graph
        .answered_in_inbox(req.id)
        .map_err(|e| for_agent(internal(e)))?;
    Ok(Json(QuestionState { answered }))
}

pub(super) async fn agent_take(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<TakeRequest>,
) -> Result<Json<Taken>, AgentError> {
    let node = app.caller(&headers)?;
    app.idle.turn_started(node, req.hook);
    match req.turn {
        Some(turn) if req.human_input => app.idle.prompted(node, turn),
        Some(turn) => app.idle.stepped(node, turn),
        None => {}
    }
    keep_transcript(&app, node, req.transcript.as_deref())?;
    let messages = app
        .graph
        .take(node, &req.kinds, req.limit_bytes)
        .map_err(for_agent)?;
    let note = match req.human_input {
        true => app
            .human_input(node, req.prompt.as_deref())
            .map_err(for_agent)?,
        false => None,
    };
    Ok(Json(Taken { messages, note }))
}

pub(super) async fn agent_idle(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<Option<IdleRequest>>,
) -> Result<Json<Vec<Delivery>>, AgentError> {
    let node = app.caller(&headers)?;
    let req = req.unwrap_or_default();
    keep_transcript(&app, node, req.transcript.as_deref())?;
    app.idle.stop_reached(node, req.turn);
    let mut wait = app.idle.open(node);
    let alive = match app.graph.relations(node) {
        Ok(relations) => relations.me.alive,
        Err(e) => {
            wait.finish(IdleEnd::Error);
            return Err(for_agent(e));
        }
    };
    if !alive {
        app.idle.end(node, IdleEnd::Exit);
        wait.finish(IdleEnd::Exit);
        eprintln!(
            "loom: 노드 {}의 쉬는 대기를 거절했다: 프로세스가 끝났다",
            node.0
        );
        return Err((StatusCode::GONE, "The node process has exited.".to_owned()));
    }
    if req.rate_limited {
        app.rate_limited(node, crate::protocol::Provider::Claude);
    }
    app.turn_ended(node);
    while app.relaunches.holds_mail(node) {
        let by = wait.ended().await;
        if app.caller(&headers).is_err() || !app.idle.reopen(&mut wait) {
            wait.finish(by);
            return Ok(Json(Vec::new()));
        }
    }
    tokio::select! {
        taken = app.next_mail(node, Kind::ALL, claude::HOOK_LIMIT_BYTES) => match taken {
            Ok(taken) => {
                wait.finish(IdleEnd::Mail);
                Ok(Json(taken))
            }
            Err(e) => {
                wait.finish(IdleEnd::Error);
                eprintln!("loom: 노드 {}의 쉬는 대기가 끝났다: {e}", node.0);
                Err(for_agent(e))
            }
        },
        by = wait.ended() => {
            wait.finish(by);
            Ok(Json(Vec::new()))
        }
    }
}

pub(super) async fn agent_interrupt_pending(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
) -> Result<Json<bool>, AgentError> {
    let node = app.caller(&headers)?;
    let pending = app.graph.interrupt_pending(node);
    Ok(Json(pending.map_err(|e| for_agent(Refusal::from(e)))?))
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

pub(super) async fn agent_guard(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<GuardRequest>,
) -> Result<Json<()>, AgentError> {
    let node = app.caller(&headers)?;
    if app.node_wsl(Some(node)).map_err(for_agent)?.is_none() {
        app.host.guard(node, req.caller);
    }
    Ok(Json(()))
}

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

pub(super) async fn agent_said(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<SaidRequest>,
) -> Result<Json<()>, AgentError> {
    let node = app.caller(&headers)?;
    app.said(node, req.text, req.turn);
    Ok(Json(()))
}

pub(super) async fn agent_question_box(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
) -> Result<Json<()>, AgentError> {
    let node = app.caller(&headers)?;
    app.turn_ends.question_box(node);
    Ok(Json(()))
}

pub(super) async fn agent_turn_end(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<Option<TurnEndRequest>>,
) -> Result<Json<Option<String>>, AgentError> {
    let node = app.caller(&headers)?;
    app.idle.stop_reached(node, req.unwrap_or_default().turn);
    Ok(Json(app.turn_end(node).map(str::to_owned)))
}

async fn agent_hook_wait(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<HookWaitRequest>,
) -> Result<Json<()>, AgentError> {
    app.caller(&headers)?;
    app.graph
        .set_wait(
            bearer(&headers)?,
            WaitSource::Hook,
            req.wait.as_ref().map(|(kind, text)| (*kind, text.as_str())),
        )
        .map_err(|e| for_agent(Refusal::from(e)))?;
    Ok(Json(()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_agent_route_the_cli_calls_is_on_the_node_door() {
        let cli = include_str!("../cli.rs");
        let daemon = include_str!("commands.rs");
        let door = &daemon[daemon.find("fn node_routes()").unwrap()..];
        let door = &door[..door.find("fn routes()").unwrap()];
        let mut names = std::collections::BTreeSet::new();
        for part in cli.split("call(").skip(1) {
            let part = part.trim_start();
            if let Some(rest) = part.strip_prefix('"')
                && let Some((name, _)) = rest.split_once('"')
                && name.chars().all(|c| c.is_ascii_lowercase() || c == '-')
            {
                names.insert(name);
            }
        }
        assert!(
            names.contains("task") && names.contains("verdict"),
            "{names:?}"
        );
        for name in names {
            assert!(
                door.contains(&format!("\"/api/agent/{name}\"")),
                "the CLI calls {name}, but the node door has no /api/agent/{name}"
            );
        }
    }
}
