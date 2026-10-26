use std::future::Future;
use std::net::IpAddr;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use std::sync::Arc;

use tokio::sync::watch;

use std::path::Path;

use portable_pty::CommandBuilder;

use super::adapter::{Command, Facts, NoServer};
use super::{Change, NotTaken, Remote, ServerAdapter, Signal, Status, VersionMismatch};
use crate::protocol::{Delivery, Kind, Model, ModelChoice, Provider, WaitKind};
use crate::screen::{Answer, Dialog, Grammar, Options, Place, Top};
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

pub struct Codex;

impl super::adapter::Adapter for Codex {
    fn facts(&self) -> Facts {
        Facts {
            provider: Some(Provider::Codex),
            hooked: false,
            listens_at_start: true,
            pinned: Some(CMD),
            usage_part: "usage/codex",
            waiting: WAITING,
        }
    }

    fn leading_args(&self, resume: Option<&str>, bypass: &[String]) -> Vec<String> {
        match resume {
            Some(thread) => vec!["resume".into(), thread.into()],
            None => bypass.to_vec(),
        }
    }

    fn command(&self, cmd: &mut CommandBuilder, at: Command<'_>) -> Result<(), NoServer> {
        let Some(Remote::Codex { url }) = at.remote else {
            return Err(NoServer);
        };
        cmd.args(["--remote", url]);
        Ok(())
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
}

pub fn server_args(launch: Launch) -> Vec<String> {
    let usage = serde_json::to_string(&usage(launch)).expect("문자열은 늘 직렬화된다");
    vec!["-c".into(), format!("developer_instructions={usage}")]
}

pub async fn resume<F, Fut>(call: F, thread: &str, bypass: bool) -> Result<(), String>
where
    F: Fn(&'static str, Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    let (sandbox, approval) = match bypass {
        true => ("danger-full-access", "never"),
        false => ("workspace-write", "on-request"),
    };
    let params = serde_json::json!({
        "threadId": thread,
        "sandbox": sandbox,
        "approvalPolicy": approval,
        "excludeTurns": true,
    });
    call("thread/resume", params).await.map(drop)
}

pub fn sandbox_args(version_output: &str, daemon: IpAddr) -> Vec<String> {
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

pub fn installed_version() -> Option<String> {
    super::installed_version(CMD)
}

pub async fn model<F, Fut>(call: F, thread: &str) -> Result<Model, String>
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

pub async fn add_skills<F, Fut>(call: F, skills: &std::path::Path) -> Result<(), String>
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

pub fn kinds(status: Status) -> &'static [&'static [Kind]] {
    match status {
        Status::Idle => &[Kind::ALL],
        Status::Busy => &[&[Kind::Steer, Kind::Interrupt]],
    }
}

pub fn adapter(server: Arc<CodexServer>) -> impl ServerAdapter {
    let thread = server.thread();
    let stopper = server.clone();
    Adapter::new(
        move |method, params| {
            let server = server.clone();
            async move { server.call(method, params).await }
        },
        thread,
        move || {
            stopper.stop_shells();
        },
    )
}

pub struct Adapter<F, S> {
    call: F,
    stop: S,
    thread: watch::Receiver<Option<server::Thread>>,
}

impl<F, S> Adapter<F, S> {
    pub fn new(call: F, thread: watch::Receiver<Option<server::Thread>>, stop: S) -> Self {
        Self { call, stop, thread }
    }

    fn thread(&self) -> Result<String, String> {
        self.thread
            .borrow()
            .as_ref()
            .map(|t| t.id.clone())
            .ok_or_else(|| "Codex 스레드가 아직 없다".to_owned())
    }
}

impl<F, Fut, S> ServerAdapter for Adapter<F, S>
where
    F: Fn(&'static str, Value) -> Fut + Send + Sync,
    Fut: Future<Output = Result<Value, String>> + Send,
    S: Fn() + Send + Sync,
{
    fn takes(&self, now: Status) -> &'static [&'static [Kind]] {
        self.thread().map_or(&[], |_| kinds(now))
    }

    fn status(&mut self) -> Option<Status> {
        self.thread.borrow_and_update().as_ref().map(|t| t.status)
    }

    async fn signal(&mut self, change: Option<Change>) -> Option<Signal> {
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
        let reply = match last_reply(&self.call, &thread).await {
            Ok(reply) => reply,
            Err(e) => {
                eprintln!("loom: Codex 스레드 {thread}의 답 글을 읽지 못했다: {e}");
                None
            }
        };
        Some(Signal::TurnEnded(reply))
    }

    async fn changed(&mut self) -> bool {
        self.thread.changed().await.is_ok()
    }

    async fn inject(&mut self, taken: &[Delivery], now: Status) -> Result<(), NotTaken> {
        let thread = self.thread().map_err(NotTaken::EventFollows)?;
        deliver(&self.call, &thread, now, taken)
            .await
            .map_err(NotTaken::EventFollows)
    }

    async fn stop_shells(&self) {
        (self.stop)();
    }

    async fn start_turn(&mut self, text: &str) -> Result<(), String> {
        start_turn(&self.call, &self.thread()?, text).await
    }

    async fn steer(&mut self, text: &str) -> Result<(), String> {
        steer(&self.call, &self.thread()?, text).await
    }
}

pub async fn last_reply<F, Fut>(call: F, thread: &str) -> Result<Option<String>, String>
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

pub async fn start_turn<F, Fut>(call: F, thread: &str, text: &str) -> Result<(), String>
where
    F: Fn(&'static str, Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    Rpc { call, thread }.start(input(text.to_owned())).await
}

pub async fn steer<F, Fut>(call: F, thread: &str, text: &str) -> Result<(), String>
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

pub async fn deliver<F, Fut>(
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
