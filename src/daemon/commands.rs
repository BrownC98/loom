use super::runtime::bearer;
use super::*;
use crate::process_tree::Identity;

pub(super) fn node_routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/agent/note", post(agent_note))
        .route("/api/agent/wait", post(agent_wait))
        .route("/api/agent/answer", post(agent_answer))
        .route("/api/agent/status", post(agent_status))
        .route("/api/agent/nodes", post(agent_nodes))
        .route("/api/agent/take", post(agent_take))
        .route(IDLE_PATH, post(agent_idle))
        .route("/api/agent/ack", post(agent_ack))
        .route("/api/agent/interrupt", post(agent_interrupt))
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
        .route("/api/agent/report", post(agent_report))
        .route("/api/agent/turn-end", post(agent_turn_end))
        .route("/api/agent/adopt", post(agent_adopt))
        .route("/api/agent/detach", post(agent_detach))
        .route("/api/agent/rm", post(agent_remove))
        .route("/api/agent/relaunch", post(relaunch::agent_relaunch))
        .route(
            "/api/agent/relaunch-unserved",
            post(relaunch::agent_relaunch_unserved),
        )
        .route("/api/agent/ready", post(gate::agent_ready))
        .route("/api/agent/verdict", post(gate::agent_verdict))
        .route("/api/agent/task", post(agent_task))
}

pub(super) const IDLE_PATH: &str = "/api/agent/idle";

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
}

impl SpawnRun {
    pub fn name(&self) -> String {
        match self {
            SpawnRun::Role { name, .. } => name.clone(),
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
    #[serde(default, rename = "transcript")]
    pub session: Option<Session>,
    #[serde(default)]
    pub human_input: bool,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub turn: Option<String>,
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
    #[serde(rename = "transcript")]
    pub session: Option<Session>,
    pub rate_limited: bool,
    pub turn_end: Option<EndedTurn>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EndedTurn {
    pub reply: Option<String>,
    #[serde(default)]
    pub turn: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum IdleReply {
    Mail(Vec<Delivery>),
    Nudge { nudge: String },
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum GuardRequest {
    Pc { caller: u32 },
    Inside { shell: u32, birth: i64 },
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
pub struct DetachRequest {
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

#[derive(Deserialize)]
struct SaidRequest {
    text: String,
    #[serde(default)]
    turn: Option<String>,
}

#[derive(Deserialize)]
struct HookWaitRequest {
    wait: Option<(WaitKind, String)>,
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
    #[serde(default)]
    pub keep: Vec<Kept>,
}

#[derive(Serialize, Deserialize)]
pub struct Reported {
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
    app.take_now(node, bearer(&headers)?, req)
        .map(Json)
        .map_err(for_agent)
}

pub(super) async fn agent_idle(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    arrival: Option<axum::Extension<arrival::Arrival>>,
    Json(req): Json<Option<IdleRequest>>,
) -> Result<Json<IdleReply>, AgentError> {
    let node = app.caller(&headers)?;
    let current = || app.caller(&headers).is_ok();
    let arrival = arrival.map(|axum::Extension(arrival)| arrival);
    app.wait_idle(
        node,
        bearer(&headers)?,
        req.unwrap_or_default(),
        arrival,
        current,
    )
    .await
    .map(Json)
}

async fn agent_interrupt(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
) -> Result<Json<()>, AgentError> {
    let node = app.caller(&headers)?;
    app.next_mail(node, &[Kind::Interrupt], crate::profile::BATCH_LIMIT_BYTES)
        .await
        .map_err(for_agent)?;
    Ok(Json(()))
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
    let shell = match (req, app.node_wsl(Some(node)).map_err(for_agent)?) {
        (GuardRequest::Pc { caller }, None) => app.host.guarded_shell(node, caller),
        (GuardRequest::Inside { shell, birth }, Some(_)) => Some(Identity { pid: shell, birth }),
        _ => None,
    };
    if let Some(shell) = shell {
        app.graph
            .guard(node, shell)
            .map_err(|e| for_agent(Refusal::from(e)))?;
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

async fn agent_said(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<SaidRequest>,
) -> Result<Json<()>, AgentError> {
    let node = app.caller(&headers)?;
    app.turn_ends.said(node, &req.text, req.turn.as_deref());
    Ok(Json(()))
}

async fn agent_question_box(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
) -> Result<Json<()>, AgentError> {
    let node = app.caller(&headers)?;
    app.turn_ends.question_box(node);
    Ok(Json(()))
}

async fn agent_report(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(report): Json<crate::protocol::Report>,
) -> Result<Json<()>, AgentError> {
    let node = app.caller(&headers)?;
    app.report(node, bearer(&headers)?, WaitSource::Hook, report);
    Ok(Json(()))
}

pub(super) async fn agent_turn_end(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
) -> Result<Json<Option<String>>, AgentError> {
    let node = app.caller(&headers)?;
    app.feed(node, AgentEvent::TurnEnded);
    let nudge = app
        .turn_ended(node, EndedTurn::default())
        .map_err(|e| for_agent(Refusal::from(e)))?;
    Ok(Json(nudge))
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
            req.wait
                .as_ref()
                .map(|(kind, text)| (*kind, text.as_str(), None)),
        )
        .map_err(|e| for_agent(Refusal::from(e)))?;
    Ok(Json(()))
}
