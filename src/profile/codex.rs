use std::future::Future;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::future::BoxFuture;
use portable_pty::CommandBuilder;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, watch};

use super::adapter::{Carry, Chosen, Command, Facts, InputSeen, ResumeError, Serve, Server};
use super::process::Served;
use super::{Change, NotTaken, ServerAdapter, Signal, Status, VersionMismatch};
use crate::accounts::{AccountId, Site};
use crate::protocol::{
    Delivery, Kind, LaunchError, Model, ModelChoice, Prompt, Provider, Report, ServerRecord,
    ServerState, Session, WaitKind,
};
use crate::screen::{Answer, Dialog, Grammar, Options, Place, Screen, Top};
use crate::skills::Launch;
use server::CodexServer;

pub mod server;

#[allow(clippy::all, dead_code)]
pub mod protocol {
    include!(concat!(env!("OUT_DIR"), "/codex_protocol.rs"));
}

use protocol::{
    AbsolutePathBuf, SkillsExtraRootsSetParams, TextUserInputType, Thread, ThreadItem,
    ThreadReadParams, ThreadReadResponse, ThreadTurnsListParams, ThreadTurnsListResponse,
    TurnInterruptParams, TurnItemsView, TurnStartParams, TurnStatus, TurnSteerParams, UserInput,
};

pub const PROMPT: Grammar = Grammar {
    top: Top::Line("Would you like to"),
    options: Options::Numbered,
    guard: None,
};

pub const PINNED_VERSION: &str = "0.159.2";
pub const DIALOGS: [Dialog; 2] = [
    Dialog {
        text: "Trust this folder?",
        kind: WaitKind::Trust,
        place: Place::BeforeSession,
        answer: Answer::Below(Options::Numbered),
    },
    Dialog {
        text: "Sign in with ChatGPT",
        kind: WaitKind::Auth,
        place: Place::BeforeSession,
        answer: Answer::Plain,
    },
];

pub(super) const WAITING: &str = "When waiting for children and no independent work remains, end your turn. Their messages start a new turn automatically. Do not run an unbounded loom wait.";

fn usage(launch: Launch) -> String {
    format!(
        "{}\n{WAITING}\n\n{}",
        super::COMMUNICATION_USAGE,
        crate::skills::always_on(launch)
    )
}

pub const CMD: &str = "codex";
const DATE_DIRS: usize = 3;
const ROLLOUT_DEPTH: usize = DATE_DIRS + 2;

pub struct Codex;

impl super::adapter::Adapter for Codex {
    fn facts(&self) -> Facts {
        Facts {
            provider: Some(Provider::Codex),
            sleeps: true,
            listens_at_start: true,
            pinned: Some(CMD),
            usage_part: "usage/codex",
            waiting: WAITING,
        }
    }

    fn leading_args(&self, resume: Option<&Session>, bypass: &[String]) -> Vec<String> {
        match resume {
            Some(thread) => vec!["resume".into(), thread.0.clone()],
            None => bypass.to_vec(),
        }
    }

    fn command(&self, _: &mut CommandBuilder, _: Command<'_>) {}

    fn serve<'a>(
        &'a self,
        at: &'a Serve<'_>,
        resume: Option<&'a Session>,
        bypass: &'a [String],
        cmd: &'a mut CommandBuilder,
    ) -> Option<BoxFuture<'a, Result<Arc<dyn Server>, LaunchError>>> {
        Some(Box::pin(async move {
            let config: Vec<String> = sandbox_args(at.version, at.node_ip)
                .into_iter()
                .chain(server_args(at.launch))
                .collect();
            let server = CodexServer::start(at.cwd, at.inside, at.env, &config)
                .await
                .map_err(|cause| LaunchError::Server {
                    program: "codex app-server".into(),
                    cause,
                })?;
            let call = |method, params| server.call(method, params);
            if let Err(e) = add_skills(call, at.skills).await {
                eprintln!("loom: Codex 서버에 내장 스킬을 더하지 못했다: {e}");
            }
            cmd.args(["--remote", server.url()]);
            let node = Node {
                server,
                resume: resume.map(|thread| (thread.clone(), resume_params(thread, bypass))),
            };
            Ok(Arc::new(node) as Arc<dyn Server>)
        }))
    }

    fn reconnect<'a>(
        &'a self,
        state: &'a ServerState,
        served: Served,
        session: Option<&'a Session>,
    ) -> Option<BoxFuture<'a, Result<Arc<dyn Server>, String>>> {
        Some(Box::pin(async move {
            let state = State::deserialize(&state.0).map_err(|e| format!("server state: {e}"))?;
            let server = CodexServer::reconnect(state.url, served, session).await?;
            Ok(Arc::new(Node {
                server,
                resume: None,
            }) as Arc<dyn Server>)
        }))
    }

    fn choice_args(&self, choice: &ModelChoice) -> Option<Vec<String>> {
        let mut args = Vec::new();
        if let Some(model) = &choice.model {
            args.extend(["-m".into(), model.clone()]);
        }
        if let Some(effort) = &choice.effort {
            let effort = serde_json::to_string(effort).expect("문자열은 늘 직렬화된다");
            args.extend(["-c".into(), format!("model_reasoning_effort={effort}")]);
        }
        Some(args)
    }

    fn screen(&self) -> Option<(&'static [Grammar], &'static [Dialog])> {
        Some((&[PROMPT], &DIALOGS))
    }

    fn input_seen(&self, _: &Screen) -> Option<InputSeen> {
        None
    }

    fn install(&self, _: &Path) -> std::io::Result<()> {
        Ok(())
    }

    fn version_warning(&self, version_output: &str) -> Option<VersionMismatch> {
        let installed = version_output.split_whitespace().last()?;
        (installed != PINNED_VERSION).then(|| VersionMismatch {
            installed: installed.to_owned(),
            pinned: PINNED_VERSION.to_owned(),
        })
    }

    fn carry(&self, thread: &Session, to: &Carry<'_>) -> Result<(), ResumeError> {
        let root = |account| {
            to.site
                .login_dir(Provider::Codex, account)
                .ok_or("홈 폴더를 찾지 못했다".to_owned())
        };
        let (old_root, new_root) = (root(to.from)?, root(to.to)?);
        if old_root == new_root {
            return Ok(());
        }
        let rollout = rollout(&old_root, &thread.0)
            .ok_or_else(|| format!("스레드 {}의 기록 파일이 없다", thread.0))?;
        Ok(super::copy_session(&rollout, ROLLOUT_DEPTH, &new_root)?)
    }

    fn end_detached<'a>(
        &'a self,
        _: &'a Session,
        _: &'a Site,
        _: Option<&'a AccountId>,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }

    fn record(&self, _: &Session) -> Option<PathBuf> {
        None
    }
}

fn rollout(root: &Path, thread: &str) -> Option<PathBuf> {
    let name = format!("-{thread}.jsonl");
    let mut dirs = vec![root.join("sessions")];
    for _ in 0..DATE_DIRS {
        dirs = dirs
            .iter()
            .flat_map(|dir| children(dir))
            .filter(|path| path.is_dir())
            .collect();
    }
    dirs.iter()
        .flat_map(|dir| children(dir))
        .find(|file| file.to_string_lossy().ends_with(&name))
}

fn children(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .collect()
}

fn server_args(launch: Launch) -> Vec<String> {
    let usage = serde_json::to_string(&usage(launch)).expect("문자열은 늘 직렬화된다");
    vec!["-c".into(), format!("developer_instructions={usage}")]
}

fn resume_params(thread: &Session, bypass: &[String]) -> Value {
    let (sandbox, approval) = match bypass.is_empty() {
        false => ("danger-full-access", "never"),
        true => ("workspace-write", "on-request"),
    };
    serde_json::json!({
        "threadId": thread.0,
        "sandbox": sandbox,
        "approvalPolicy": approval,
        "excludeTurns": true,
    })
}

fn sandbox_args(version_output: Option<&str>, daemon: Option<IpAddr>) -> Vec<String> {
    let (Some(version_output), Some(daemon)) = (version_output, daemon) else {
        return Vec::new();
    };
    if cfg!(windows) || version_output.split_whitespace().last() != Some(PINNED_VERSION) {
        return Vec::new();
    }
    [
        "sandbox_workspace_write.network_access=true".into(),
        "features.network_proxy.enabled=true".into(),
        format!(r#"features.network_proxy.domains={{"{daemon}"="allow"}}"#),
    ]
    .into_iter()
    .flat_map(|config| ["-c".into(), config])
    .collect()
}

async fn model<F, Fut>(call: F, thread: &str) -> Result<Model, String>
where
    F: Fn(&'static str, Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    let rpc = Rpc { call, thread };
    let params = ThreadReadParams {
        thread_id: thread.to_owned(),
        include_turns: Some(false),
    };
    let read: ThreadReadResponse = rpc.send("thread/read", &params).await?;
    Ok(thread_model(&read.thread))
}

async fn add_skills<F, Fut>(call: F, skills: &std::path::Path) -> Result<(), String>
where
    F: Fn(&'static str, Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    let params = SkillsExtraRootsSetParams {
        extra_roots: vec![AbsolutePathBuf(skills.to_string_lossy().into_owned())],
    };
    let params = serde_json::to_value(params).expect("요청 타입은 늘 직렬화된다");
    call("skills/extraRoots/set", params).await.map(drop)
}

fn thread_model(thread: &Thread) -> Model {
    Model {
        model: thread.model.clone(),
        effort: thread.reasoning_effort.as_deref().cloned(),
    }
}

fn kinds(status: Status) -> &'static [&'static [Kind]] {
    match status {
        Status::Idle => &[Kind::ALL],
        Status::Busy => &[&[Kind::Steer, Kind::Interrupt]],
    }
}

#[derive(Serialize, Deserialize)]
struct State {
    url: String,
}

struct Node {
    server: CodexServer,
    resume: Option<(Session, Value)>,
}

impl Server for Node {
    fn record(&self) -> ServerRecord {
        let state = State {
            url: self.server.url().to_owned(),
        };
        ServerRecord {
            pid: self.server.pid(),
            state: ServerState(serde_json::to_value(state).expect("문자열 표는 늘 직렬화된다")),
        }
    }

    fn session(&self) -> Option<Session> {
        None
    }

    fn stop(&self) {
        self.server.stop();
    }

    fn resume(&self) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let Some((thread, params)) = &self.resume else {
                return Ok(());
            };
            self.server.call("thread/resume", params.clone()).await?;
            self.server.follow_thread(thread.0.clone()).await;
            Ok(())
        })
    }

    fn session_file<'a>(&'a self, session: &'a Session) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move { self.server.rollout(&session.0).await.ok().flatten() })
    }

    fn choose<'a>(
        &'a self,
        _: &'a Prompt,
        _: usize,
    ) -> Option<BoxFuture<'a, Result<Chosen, String>>> {
        None
    }

    fn link(self: Arc<Self>) -> Box<dyn ServerAdapter> {
        let (read, models) = mpsc::unbounded_channel();
        Box::new(Adapter {
            thread: self.server.thread(),
            limited: self.server.limited(),
            node: self,
            moved: true,
            limit_hit: false,
            fresh: None,
            read,
            models,
            model: None,
        })
    }
}

struct Adapter {
    node: Arc<Node>,
    thread: watch::Receiver<Option<server::Thread>>,
    limited: watch::Receiver<u64>,
    moved: bool,
    limit_hit: bool,
    fresh: Option<server::Thread>,
    read: mpsc::UnboundedSender<Model>,
    models: mpsc::UnboundedReceiver<Model>,
    model: Option<Model>,
}

impl Adapter {
    fn thread(&self) -> Result<String, String> {
        self.thread
            .borrow()
            .as_ref()
            .map(|t| t.id.clone())
            .ok_or_else(|| "Codex 스레드가 아직 없다".to_owned())
    }

    async fn turn_signal(&self, change: Option<Change>) -> Option<Signal> {
        match change? {
            Change {
                before: Status::Idle,
                now: Status::Busy,
            } => return Some(Signal::HumanInput),
            Change {
                before: Status::Busy,
                now: Status::Idle,
            } => {}
            Change { .. } => return None,
        }
        let thread = self.thread().ok()?;
        let call = |method, params| self.node.server.call(method, params);
        let reply = match last_reply(call, &thread).await {
            Ok(reply) => reply,
            Err(e) => {
                eprintln!("loom: Codex 스레드 {thread}의 답 글을 읽지 못했다: {e}");
                None
            }
        };
        Some(Signal::TurnEnded(reply))
    }
}

impl ServerAdapter for Adapter {
    fn takes(&self, now: Status) -> &'static [&'static [Kind]] {
        self.thread().map_or(&[], |_| kinds(now))
    }

    fn status(&mut self) -> Option<Status> {
        if !std::mem::take(&mut self.moved) {
            return None;
        }
        self.fresh = self.thread.borrow_and_update().clone();
        self.fresh.as_ref().map(|t| t.status)
    }

    fn signals(&mut self, change: Option<Change>) -> BoxFuture<'_, Vec<Signal>> {
        Box::pin(async move {
            let mut signals = Vec::new();
            if std::mem::take(&mut self.limit_hit) {
                signals.push(Signal::Report(Report::RateLimited));
            }
            if let Some(model) = self.model.take() {
                signals.push(Signal::Report(Report::Model(model)));
            }
            if let Some(thread) = self.fresh.take() {
                let wait = thread.waiting.map(|kind| (kind, String::new(), None));
                signals.push(Signal::Report(Report::HumanWait(wait)));
                signals.push(Signal::Report(Report::Session(Session(thread.id.clone()))));
                let (node, read) = (self.node.clone(), self.read.clone());
                tokio::spawn(async move {
                    let call = |method, params| node.server.call(method, params);
                    match model(call, &thread.id).await {
                        Ok(model) => drop(read.send(model)),
                        Err(e) => {
                            eprintln!("loom: Codex 스레드 {}의 모델을 읽지 못했다: {e}", thread.id)
                        }
                    }
                });
            }
            signals.extend(self.turn_signal(change).await);
            signals
        })
    }

    fn changed(&mut self) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            tokio::select! {
                moved = self.thread.changed() => {
                    self.moved = true;
                    moved.is_ok()
                }
                hit = self.limited.changed() => {
                    self.limit_hit = hit.is_ok();
                    hit.is_ok()
                }
                Some(model) = self.models.recv() => {
                    self.model = Some(model);
                    true
                }
            }
        })
    }

    fn inject<'a>(
        &'a mut self,
        taken: &'a [Delivery],
        now: Status,
    ) -> BoxFuture<'a, Result<(), NotTaken>> {
        Box::pin(async move {
            let thread = self.thread().map_err(NotTaken::EventFollows)?;
            let call = |method, params| self.node.server.call(method, params);
            deliver(call, &thread, now, taken)
                .await
                .map_err(NotTaken::EventFollows)
        })
    }

    fn stop_shells(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.node.server.stop_shells();
        })
    }

    fn start_turn<'a>(&'a mut self, text: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let call = |method, params| self.node.server.call(method, params);
            start_turn(call, &self.thread()?, text).await
        })
    }

    fn steer<'a>(&'a mut self, text: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let call = |method, params| self.node.server.call(method, params);
            steer(call, &self.thread()?, text).await
        })
    }
}

async fn last_reply<F, Fut>(call: F, thread: &str) -> Result<Option<String>, String>
where
    F: Fn(&'static str, Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    let rpc = Rpc { call, thread };
    let list: ThreadTurnsListResponse = rpc
        .send("thread/turns/list", &rpc.last_turn(TurnItemsView::Full))
        .await?;
    Ok(list.data.into_iter().next().and_then(|turn| {
        turn.items.into_iter().rev().find_map(|item| match item {
            ThreadItem::AgentMessage { text, .. } => Some(text),
            _ => None,
        })
    }))
}

async fn start_turn<F, Fut>(call: F, thread: &str, text: &str) -> Result<(), String>
where
    F: Fn(&'static str, Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    Rpc { call, thread }.start(input(text.to_owned())).await
}

async fn steer<F, Fut>(call: F, thread: &str, text: &str) -> Result<(), String>
where
    F: Fn(&'static str, Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    Rpc { call, thread }.steer(input(text.to_owned())).await
}

fn input(text: String) -> Vec<UserInput> {
    vec![UserInput::TextUserInput {
        text,
        text_elements: Vec::new(),
        type_: TextUserInputType::Text,
    }]
}

async fn deliver<F, Fut>(
    call: F,
    thread: &str,
    status: Status,
    taken: &[Delivery],
) -> Result<(), String>
where
    F: Fn(&'static str, Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    let rpc = Rpc { call, thread };
    let input = input(super::message_text(taken));
    match status {
        Status::Idle => rpc.start_or_steer(input).await,
        Status::Busy if taken.iter().any(|d| d.kind == Kind::Interrupt) => {
            if let Some(turn) = rpc.current_turn().await? {
                let params = TurnInterruptParams {
                    thread_id: thread.to_owned(),
                    turn_id: turn,
                };
                let _ = rpc.send::<Value>("turn/interrupt", &params).await;
            }
            rpc.start_or_steer(input).await
        }
        Status::Busy => match rpc.steer(input.clone()).await {
            Ok(()) => Ok(()),
            Err(_) => rpc.start(input).await,
        },
    }
}

struct Rpc<'a, F> {
    call: F,
    thread: &'a str,
}

impl<F, Fut> Rpc<'_, F>
where
    F: Fn(&'static str, Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    async fn send<R: DeserializeOwned>(
        &self,
        method: &'static str,
        params: &impl Serialize,
    ) -> Result<R, String> {
        let params = serde_json::to_value(params).expect("요청 타입은 늘 직렬화된다");
        let result = (self.call)(method, params).await?;
        serde_json::from_value(result).map_err(|e| format!("{method}: 알 수 없는 응답이다: {e}"))
    }

    async fn start_or_steer(&self, input: Vec<UserInput>) -> Result<(), String> {
        match self.start(input.clone()).await {
            Ok(()) => Ok(()),
            Err(start) => self
                .steer(input)
                .await
                .map_err(|steer| format!("{start} / {steer}")),
        }
    }

    async fn start(&self, input: Vec<UserInput>) -> Result<(), String> {
        let params = TurnStartParams {
            thread_id: self.thread.to_owned(),
            input,
            approval_policy: None,
            approvals_reviewer: None,
            client_user_message_id: None,
            cwd: None,
            disabled_plugin_ids: None,
            effort: None,
            model: None,
            output_schema: None,
            personality: None,
            sandbox_policy: None,
            service_tier: None,
            service_tier_for_turn: None,
            summary: None,
            tool_output: None,
            turn_trigger: None,
        };
        self.send::<Value>("turn/start", &params).await.map(drop)
    }

    async fn steer(&self, input: Vec<UserInput>) -> Result<(), String> {
        let turn = self
            .current_turn()
            .await?
            .ok_or("turn/steer: 진행 중인 턴이 없다")?;
        let params = TurnSteerParams {
            thread_id: self.thread.to_owned(),
            expected_turn_id: turn,
            input,
            client_user_message_id: None,
        };
        self.send::<Value>("turn/steer", &params).await.map(drop)
    }

    fn last_turn(&self, items: TurnItemsView) -> ThreadTurnsListParams {
        ThreadTurnsListParams {
            thread_id: self.thread.to_owned(),
            limit: Some(1),
            cursor: None,
            items_view: Some(items),
            sort_direction: None,
        }
    }

    async fn current_turn(&self) -> Result<Option<String>, String> {
        let params = self.last_turn(TurnItemsView::NotLoaded);
        let list: ThreadTurnsListResponse = self.send("thread/turns/list", &params).await?;
        Ok(list
            .data
            .into_iter()
            .next()
            .filter(|t| t.status == TurnStatus::InProgress)
            .map(|t| t.id))
    }
}
