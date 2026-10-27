use super::runtime::bearer;
use super::*;
use crate::protocol::ChoiceRequest;

pub(super) fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/waits/{wait}", delete(dismiss_wait))
        .route("/api/waits/{wait}/answer", post(answer_wait))
        .route("/api/waits/{wait}/choose", post(choose_wait))
        .route("/api/waits/{wait}/type", post(type_wait))
        .route("/api/docs", get(list_docs))
        .route(
            "/api/docs/{id}",
            post(open_doc).layer(axum::middleware::from_fn(origin::from_page)),
        )
        .route("/api/docs/{id}/", get(doc))
        .route("/api/docs/{id}/{*file}", get(doc_sibling))
        .route("/api/waits/{wait}/images/{index}", get(done_image_file))
}

/// 사람이 화면에서 `loom ask` 질문에 쓴 답. `images`는 답에 붙인 그림의 데몬 기계 경로다(`POST /api/pastes`가 준 값, 목업 M30).
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct AnswerWait {
    pub text: String,
    #[serde(default)]
    pub images: Vec<PathBuf>,
}

/// 사람이 알림에서 고른 승인 창 선택지. `prompt`는 화면이 보인 창 그대로, `option`은 그 `options`의 차례다.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct ChooseOption {
    pub prompt: Prompt,
    #[ts(type = "number")]
    pub option: usize,
}

/// 사람이 알림의 입력칸에 쓴 글. 데몬은 `prompt`가 지금 화면과 같을 때만 그 글과 Enter를 치고, 글을 저장하거나 로그에 쓰지 않는다.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct TypeText {
    pub prompt: Prompt,
    pub text: String,
    /// 글로 답하는 선택지(`PromptOption::writes`)의 차례. 없으면 창의 입력칸(`Prompt::field`)이다.
    #[serde(default)]
    #[ts(optional, type = "number")]
    pub option: Option<usize>,
}

async fn dismiss_wait(
    State(app): State<Arc<App>>,
    Path(wait): Path<u32>,
) -> Result<Json<()>, Problem> {
    app.graph.dismiss(WaitId(wait))?;
    Ok(Json(()))
}

async fn answer_wait(
    State(app): State<Arc<App>>,
    Path(wait): Path<u32>,
    Json(req): Json<AnswerWait>,
) -> Result<Json<()>, Problem> {
    let node = app.graph.answer(WaitId(wait), &req.text, &req.images)?;
    app.turn_ends.human(node);
    Ok(Json(()))
}

async fn choose_wait(
    State(app): State<Arc<App>>,
    Path(wait): Path<u32>,
    Json(req): Json<ChooseOption>,
) -> Result<Json<()>, Problem> {
    let wait = WaitId(wait);
    let (node, _) = app.graph.dialog(wait)?;
    let active = app.graph.active().map_err(internal)?;
    let found = active.iter().find(|n| n.id == node);
    let held = app.choices.choose(node, &req.prompt, req.option);
    let chosen = match (
        held,
        found.and_then(|n| Some((n, relaunch::profile_of(n)?))),
    ) {
        (Some(chosen), _) => chosen,
        (None, Some((n, profile))) => {
            let record = app.record_of(n);
            let adapter = profile::adapter(profile);
            adapter
                .answer(&record, &req.prompt, req.option)
                .await
                .map_err(internal)?
        }
        (None, None) => Chosen::OnScreen,
    };
    match chosen {
        Chosen::Sent => {}
        Chosen::PromptChanged => return Err(Refusal::PromptChanged(wait).into()),
        Chosen::OnScreen => {
            let option = req.option;
            answer_dialog(&app, wait, req.prompt, move |now| {
                now.options.get(option).filter(|o| !o.writes)?.key.picked()
            })
            .await?
        }
    }
    Ok(Json(()))
}

async fn type_wait(
    State(app): State<Arc<App>>,
    Path(wait): Path<u32>,
    Json(req): Json<TypeText>,
) -> Result<Json<()>, Problem> {
    let TypeText {
        prompt,
        text,
        option,
    } = req;
    answer_dialog(&app, WaitId(wait), prompt, move |now| match option {
        Some(option) => now
            .options
            .get(option)
            .filter(|o| o.writes)?
            .key
            .written(&text),
        None => now.field.map(|_| entered(&text)),
    })
    .await?;
    Ok(Json(()))
}

const KEY_GAP: Duration = Duration::from_millis(150);

async fn answer_dialog(
    app: &Arc<App>,
    wait: WaitId,
    shown: Prompt,
    strokes: impl FnOnce(&Prompt) -> Option<Strokes> + Send + 'static,
) -> Result<(), Problem> {
    let (node, agent) = app.graph.dialog(wait)?;
    let profile = agent.as_ref().and_then(profile::profile_of);
    let app = app.clone();
    tokio::task::spawn_blocking(move || {
        app.host.strokes_if(node, KEY_GAP, |screen| {
            let now = screen.screen();
            let strokes = profile::prompt(profile, &now)
                .filter(|now| *now == shown)
                .and_then(|now| strokes(&now))
                .filter(|s| !s.is_empty())
                .ok_or(Refusal::PromptChanged(wait))?;
            app.graph.dismiss(wait)?;
            Ok::<_, Refusal>(strokes)
        })
    })
    .await
    .map_err(internal)??;
    Ok(())
}

async fn list_docs() -> Result<Json<Vec<user_doc::DocRow>>, Problem> {
    Ok(Json(user_doc::docs().map_err(internal)?))
}

async fn open_doc(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> Result<Json<()>, DocProblem> {
    doc_file(&id)?;
    open::open_url(&format!("http://127.0.0.1:{}/api/docs/{id}/", app.port)).map_err(internal)?;
    Ok(Json(()))
}

async fn doc(
    Path(id): Path<String>,
    request: axum::extract::Request,
) -> Result<Response, DocProblem> {
    let doc = doc_file(&id)?;
    Ok(serve_file(&doc.path, request).await?)
}

async fn doc_sibling(
    Path((id, file)): Path<(String, String)>,
    request: axum::extract::Request,
) -> Result<Response, DocProblem> {
    let doc = doc_file(&id)?;
    match doc.folder.and_then(|dir| passes::inside(&dir, &file)) {
        Some(path) => Ok(serve_file(&path, request).await?),
        None => Ok(StatusCode::NOT_FOUND.into_response()),
    }
}

pub enum DocProblem {
    Missing(String),
    Other(Problem),
}

impl From<Problem> for DocProblem {
    fn from(e: Problem) -> Self {
        DocProblem::Other(e)
    }
}

impl IntoResponse for DocProblem {
    fn into_response(self) -> Response {
        match self {
            DocProblem::Missing(reason) => (
                StatusCode::NOT_FOUND,
                [(axum::http::header::CONTENT_TYPE, TEXT_TYPE)],
                reason,
            )
                .into_response(),
            DocProblem::Other(problem) => problem.into_response(),
        }
    }
}

pub(super) fn doc_file(id: &str) -> Result<user_doc::DocFile, DocProblem> {
    user_doc::doc_file(id).map_err(DocProblem::Missing)
}

async fn done_image_file(
    State(app): State<Arc<App>>,
    Path((wait, index)): Path<(u32, usize)>,
    request: axum::extract::Request,
) -> Result<Response, Problem> {
    let path = app.graph.done_image(WaitId(wait), index)?;
    serve_file(&path, request).await
}

const TEXT_TYPE: &str = "text/plain; charset=utf-8";

pub(super) async fn serve_file(
    path: &std::path::Path,
    request: axum::extract::Request,
) -> Result<Response, Problem> {
    Ok(tower_http::services::ServeFile::new(path)
        .try_call(request)
        .await
        .map_err(internal)?
        .into_response())
}

#[derive(Default)]
pub(super) struct Choices(std::sync::Mutex<HashMap<NodeId, Held>>);

struct Held {
    prompt: Prompt,
    chosen: tokio::sync::oneshot::Sender<usize>,
}

impl Choices {
    fn hold(&self, node: NodeId, prompt: Prompt) -> tokio::sync::oneshot::Receiver<usize> {
        let (chosen, receiver) = tokio::sync::oneshot::channel();
        self.0.lock().unwrap().insert(node, Held { prompt, chosen });
        receiver
    }

    fn choose(&self, node: NodeId, prompt: &Prompt, option: usize) -> Option<Chosen> {
        let mut held = self.0.lock().unwrap();
        let now = held.get(&node)?;
        if now.prompt != *prompt || option >= now.prompt.options.len() {
            return Some(Chosen::PromptChanged);
        }
        let now = held.remove(&node)?;
        Some(match now.chosen.send(option) {
            Ok(()) => Chosen::Sent,
            Err(_) => Chosen::PromptChanged,
        })
    }

    fn release(&self, node: NodeId) -> bool {
        let mut held = self.0.lock().unwrap();
        if held.get(&node).is_some_and(|h| !h.chosen.is_closed()) {
            return false;
        }
        held.remove(&node);
        true
    }
}

struct Release {
    app: Arc<App>,
    node: NodeId,
    token: String,
}

impl Drop for Release {
    fn drop(&mut self) {
        if !self.app.choices.release(self.node) {
            return;
        }
        if let Err(e) = self
            .app
            .graph
            .set_wait(&self.token, WaitSource::Agent, None)
        {
            eprintln!(
                "loom: 노드 {}의 응답 대기를 지우지 못했다: {e}",
                self.node.0
            );
        }
    }
}

pub(super) async fn agent_choice(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<ChoiceRequest>,
) -> Result<Json<usize>, AgentError> {
    let node = app.caller(&headers)?;
    let token = bearer(&headers)?.to_owned();
    let chosen = app.choices.hold(node, req.prompt.clone());
    let release = Release {
        app: app.clone(),
        node,
        token,
    };
    let summary = req.prompt.summary.clone();
    app.graph
        .set_wait(
            &release.token,
            WaitSource::Agent,
            Some((req.kind, &summary, Some(req.prompt))),
        )
        .map_err(|e| for_agent(Refusal::from(e)))?;
    let option = chosen.await.map_err(|_| {
        (
            StatusCode::CONFLICT,
            "A newer choice of this node replaced this one.".to_owned(),
        )
    })?;
    Ok(Json(option))
}
