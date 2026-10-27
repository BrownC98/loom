use std::future::Future;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use portable_pty::CommandBuilder;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, watch};

use super::VersionMismatch;
use super::adapter::{
    Carry, Chosen, Command, Facts, InputSeen, LaunchAt, Port, Record, ResumeError, Started,
};
use super::process::Served;
use crate::accounts::Site;
use crate::protocol::{
    Delivery, EndedTurn, IdleReply, IdleRequest, Kind, LaunchError, MessageId, Model, ModelChoice,
    Prompt, PromptKey, PromptOption, Provider, Refusal, Report, Session, TakeRequest, WaitKind,
};
use crate::screen::{Answer, Dialog, Grammar, Options, Place, Screen, Top};
use crate::skills::Launch;
use server::{Approval, Asked, CodexServer};

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
            helper_shells: &[],
        }
    }

    fn leading_args(&self, resume: Option<&Session>, bypass: &[String]) -> Vec<String> {
        match resume {
            Some(thread) => vec!["resume".into(), thread.0.clone()],
            None => bypass.to_vec(),
        }
    }

    fn prepare(&self, _: &LaunchAt<'_>) -> std::io::Result<()> {
        Ok(())
    }

    fn command(&self, _: &mut CommandBuilder, _: Command<'_>) {}

    fn launch<'a>(
        &'a self,
        at: &'a LaunchAt<'_>,
        resume: Option<&'a Session>,
        bypass: &'a [String],
        cmd: &'a mut CommandBuilder,
        port: Arc<dyn Port>,
    ) -> BoxFuture<'a, Result<Started, LaunchError>> {
        Box::pin(async move {
            let config: Vec<String> = sandbox_args(at.version, at.node_ip)
                .into_iter()
                .chain(server_args(at.launch))
                .collect();
            let server = CodexServer::start(at.cwd, at.inside, at.env, &config)
                .await
                .map_err(|cause| LaunchError::Helper {
                    program: "codex app-server".into(),
                    cause,
                })?;
            let call = |method, params| server.call(method, params);
            if let Err(e) = add_skills(call, at.skills).await {
                eprintln!("loom: Codex 서버에 내장 스킬을 더하지 못했다: {e}");
            }
            cmd.args(["--remote", server.url()]);
            let record = server.process().record(State {
                url: server.url().to_owned(),
            });
            let server = Arc::new(server);
            let before_start = resume.map(|thread| resume_thread(server.clone(), thread, bypass));
            tokio::spawn(drive(server, port));
            Ok(Started {
                record: Some(record),
                session: None,
                before_start,
            })
        })
    }

    fn attach<'a>(
        &'a self,
        rec: &'a Record,
        port: Arc<dyn Port>,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let Some((served, state)) = Served::stored::<State>(rec) else {
                return Ok(());
            };
            match CodexServer::reconnect(state.url, served.clone(), rec.session.as_ref()).await {
                Ok(server) => {
                    tokio::spawn(drive(Arc::new(server), port));
                    Ok(())
                }
                Err(e) => {
                    served.stop();
                    Err(e)
                }
            }
        })
    }

    fn answer<'a>(
        &'a self,
        rec: &'a Record,
        prompt: &'a Prompt,
        option: usize,
    ) -> BoxFuture<'a, Result<Chosen, String>> {
        Box::pin(async move {
            let (Some((served, state)), Some(thread)) =
                (Served::stored::<State>(rec), &rec.session)
            else {
                return Ok(Chosen::OnScreen);
            };
            let Some((_, decision)) = APPROVAL_OPTIONS.get(option) else {
                return Ok(Chosen::PromptChanged);
            };
            let server = CodexServer::reconnect(state.url, served, None).await?;
            let result = serde_json::json!({ "decision": decision });
            server
                .answer(&thread.0, |a| approval_rows(a) == prompt.rows, result)
                .await
        })
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

    fn end_detached<'a>(&'a self, _: &'a Record, _: &'a Site) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }

    fn record<'a>(&'a self, rec: &'a Record) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move {
            let (served, state) = Served::stored::<State>(rec)?;
            let thread = rec.session.as_ref()?;
            let server = CodexServer::reconnect(state.url, served, None).await.ok()?;
            server.rollout(&thread.0).await.ok().flatten()
        })
    }

    fn shell_root(&self, rec: &Record) -> Option<u32> {
        Served::stored::<State>(rec)?.0.here()
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Status {
    Idle,
    Busy,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum Decision {
    Accept,
    AcceptForSession,
    Decline,
}

impl Decision {
    fn key(self) -> PromptKey {
        let text = serde_json::to_value(self).expect("enum은 늘 직렬화된다");
        PromptKey::Reply(text.as_str().unwrap_or_default().to_owned())
    }
}

const APPROVAL_OPTIONS: [(&str, Decision); 3] = [
    ("Yes, proceed", Decision::Accept),
    (
        "Yes, and don't ask again this session",
        Decision::AcceptForSession,
    ),
    ("No, continue without it", Decision::Decline),
];

fn approval_rows(approval: &Approval) -> Vec<String> {
    vec![approval.id.to_string(), approval.item().to_owned()]
}

fn approval_prompt(approval: &Approval, files: &[String]) -> Prompt {
    let (summary, body): (String, Vec<String>) = match &approval.asked {
        Asked::Command(asked) => {
            let command = asked
                .command
                .clone()
                .unwrap_or_else(|| asked.kind.to_string());
            let body = std::iter::once(command.clone())
                .chain(asked.cwd.as_ref().map(|cwd| format!("cwd: {}", cwd.0)))
                .chain(asked.reason.clone())
                .collect();
            (command, body)
        }
        Asked::FileChange(asked) => {
            let summary = match files.is_empty() {
                true => "file change".to_owned(),
                false => format!("file change: {}", files.join(", ")),
            };
            let body = files
                .iter()
                .cloned()
                .chain(
                    asked
                        .grant_root
                        .as_ref()
                        .map(|root| format!("grant root: {root}")),
                )
                .chain(asked.reason.clone())
                .collect();
            (summary, body)
        }
    };
    Prompt {
        rows: approval_rows(approval),
        body,
        summary,
        options: APPROVAL_OPTIONS
            .iter()
            .map(|(label, decision)| PromptOption {
                label: (*label).to_owned(),
                key: decision.key(),
            })
            .collect(),
        field: None,
    }
}

async fn human_wait(
    server: &CodexServer,
    thread: Option<server::Thread>,
    approvals: Vec<Approval>,
) -> Option<(WaitKind, String, Option<Prompt>)> {
    let now = thread?;
    match approvals.iter().find(|a| a.thread() == now.id) {
        Some(approval) => {
            let files = server.files(approval).await;
            let prompt = approval_prompt(approval, &files);
            Some((WaitKind::Approval, prompt.summary.clone(), Some(prompt)))
        }
        None => now.waiting.map(|kind| (kind, String::new(), None)),
    }
}

const BUSY_KINDS: &[Kind] = &[Kind::Steer, Kind::Interrupt];
const HUMAN_STEP: &str = "human";

#[derive(Serialize, Deserialize)]
struct State {
    url: String,
}

fn resume_thread(
    server: Arc<CodexServer>,
    thread: &Session,
    bypass: &[String],
) -> BoxFuture<'static, Result<(), String>> {
    let (id, params) = (thread.0.clone(), resume_params(thread, bypass));
    Box::pin(async move {
        server.call("thread/resume", params).await?;
        server.follow_thread(id).await;
        Ok(())
    })
}

async fn pending<T>(future: &mut Option<BoxFuture<'_, T>>) -> T {
    match future {
        Some(future) => future.await,
        None => std::future::pending().await,
    }
}

async fn drive(server: Arc<CodexServer>, port: Arc<dyn Port>) {
    let mut thread = server.thread();
    let mut approvals = server.approvals();
    let mut subscribed = None;
    let mut waits = false;
    let mut limited = server.limited();
    let (read, mut models) = mpsc::unbounded_channel::<(u64, Model)>();
    let (mut asked, mut answered) = (0, 0);
    let mut status = None;
    let mut ours = false;
    let mut idle: Option<BoxFuture<'_, Result<IdleReply, String>>> = None;
    let mut busy: Option<BoxFuture<'_, Result<Vec<Delivery>, Refusal>>> = None;
    let mut ended = port.ended();
    let mut moved = true;
    loop {
        let seen = match std::mem::take(&mut moved) {
            true => thread.borrow_and_update().clone(),
            false => None,
        };
        if let Some(now) = seen {
            waits = true;
            if now.status == Status::Busy && subscribed.as_ref() != Some(&now.id) {
                match server.subscribe(&now.id).await {
                    Ok(()) => subscribed = Some(now.id.clone()),
                    Err(e) => eprintln!("loom: Codex 스레드 {}를 구독하지 못했다: {e}", now.id),
                }
            }
            port.report(Report::Session(Session(now.id.clone())));
            asked += 1;
            read_model(server.clone(), now.id.clone(), asked, read.clone());
            let before = status.replace(now.status);
            match now.status {
                Status::Busy => {
                    if before == Some(Status::Idle) && !ours {
                        human_input(&server, &*port, &now.id, &mut idle).await;
                    }
                    if before != Some(Status::Busy) {
                        port.report(Report::Busy);
                    }
                    ours = false;
                    busy.get_or_insert_with(|| port.next_mail(BUSY_KINDS));
                }
                Status::Idle => {
                    busy = None;
                    if idle.is_none() {
                        let unseen = before != Some(Status::Busy) && ours;
                        let turn_end = match before == Some(Status::Busy) || unseen {
                            true => Some(ended_turn(&server, &now.id).await),
                            false => None,
                        };
                        ours &= !unseen;
                        idle = Some(port.idle(IdleRequest {
                            turn_end,
                            ..IdleRequest::default()
                        }));
                    }
                }
            }
        }
        if std::mem::take(&mut waits) {
            let now = thread.borrow().clone();
            let asked = approvals.borrow_and_update().clone();
            port.report(Report::HumanWait(human_wait(&server, now, asked).await));
        }
        tokio::select! {
            biased;
            () = &mut ended => break,
            changed = thread.changed() => match changed {
                Ok(()) => moved = true,
                Err(_) => {
                    port.report(Report::HumanWait(None));
                    port.report(Report::Lost);
                    (&mut ended).await;
                    break;
                }
            },
            Ok(()) = approvals.changed() => waits = true,
            Ok(()) = limited.changed() => port.report(Report::RateLimited),
            Some((n, model)) = models.recv() => {
                if n > answered {
                    answered = n;
                    port.report(Report::Model(model));
                }
            }
            reply = pending(&mut idle) => {
                idle = None;
                let Some(id) = thread_id(&thread) else { continue };
                match reply {
                    Ok(IdleReply::Mail(mail)) if mail.is_empty() => {}
                    Ok(IdleReply::Mail(mail)) => {
                        let now = status.unwrap_or(Status::Idle);
                        ours |= hand(&server, &*port, &id, now, &mail).await;
                    }
                    Ok(IdleReply::Nudge { nudge }) => {
                        let call = |method, params| server.call(method, params);
                        match start_turn(call, &id, &nudge).await {
                            Ok(()) => ours = true,
                            Err(e) => eprintln!("loom: Codex 스레드 {id}를 다시 일하게 하지 못했다: {e}"),
                        }
                    }
                    Err(e) => eprintln!("loom: Codex 스레드 {id}의 쉬는 대기가 끝났다: {e}"),
                }
            }
            mail = pending(&mut busy) => {
                busy = None;
                let Some(id) = thread_id(&thread) else { continue };
                match mail {
                    Ok(mail) => {
                        let interrupt = mail.iter().any(|d| d.kind == Kind::Interrupt);
                        let handed = hand(&server, &*port, &id, Status::Busy, &mail).await;
                        ours |= handed && interrupt;
                        if handed && status == Some(Status::Busy) {
                            busy = Some(port.next_mail(BUSY_KINDS));
                        }
                    }
                    Err(e) => eprintln!("loom: Codex 스레드 {id}의 메일함을 읽지 못했다: {e}"),
                }
            }
        }
    }
    server.stop();
}

fn thread_id(thread: &watch::Receiver<Option<server::Thread>>) -> Option<String> {
    thread.borrow().as_ref().map(|t| t.id.clone())
}

fn read_model(
    server: Arc<CodexServer>,
    thread: String,
    asked: u64,
    read: mpsc::UnboundedSender<(u64, Model)>,
) {
    tokio::spawn(async move {
        let call = |method, params| server.call(method, params);
        match model(call, &thread).await {
            Ok(model) => drop(read.send((asked, model))),
            Err(e) => eprintln!("loom: Codex 스레드 {thread}의 모델을 읽지 못했다: {e}"),
        }
    });
}

async fn human_input(
    server: &CodexServer,
    port: &dyn Port,
    thread: &str,
    idle: &mut Option<BoxFuture<'_, Result<IdleReply, String>>>,
) {
    let req = TakeRequest {
        step: HUMAN_STEP.to_owned(),
        kinds: Vec::new(),
        limit_bytes: 0,
        session: None,
        human_input: true,
        prompt: None,
        turn: None,
    };
    let note = match port.take(req) {
        Ok(taken) => taken.note,
        Err(e) => {
            eprintln!("loom: Codex 스레드 {thread}의 사람 입력을 처리하지 못했다: {e}");
            return;
        }
    };
    let call = |method, params| server.call(method, params);
    match idle.take().and_then(FutureExt::now_or_never) {
        Some(Ok(IdleReply::Mail(mail))) if !mail.is_empty() => {
            hand(server, port, thread, Status::Busy, &mail).await;
        }
        Some(Ok(IdleReply::Nudge { nudge })) => {
            if let Err(e) = steer(call, thread, &nudge).await {
                eprintln!("loom: Codex 스레드 {thread}를 다시 일하게 하지 못했다: {e}");
            }
        }
        _ => {}
    }
    if let Some(note) = note
        && let Err(e) = steer(call, thread, &note).await
    {
        eprintln!("loom: Codex 스레드 {thread}에 열린 질문을 알리지 못했다: {e}");
    }
}

async fn ended_turn(server: &CodexServer, thread: &str) -> EndedTurn {
    let call = |method, params| server.call(method, params);
    let reply = last_reply(call, thread)
        .await
        .inspect_err(|e| eprintln!("loom: Codex 스레드 {thread}의 답 글을 읽지 못했다: {e}"))
        .ok()
        .flatten();
    EndedTurn { reply, turn: None }
}

async fn hand(
    server: &CodexServer,
    port: &dyn Port,
    thread: &str,
    now: Status,
    mail: &[Delivery],
) -> bool {
    let ids: Vec<MessageId> = mail.iter().map(|d| d.id).collect();
    if mail.iter().any(|d| d.kind == Kind::Interrupt) {
        server.stop_shells();
    }
    let call = |method, params| server.call(method, params);
    match deliver(call, thread, now, mail).await {
        Ok(()) => {
            if let Err(e) = port.ack(&ids) {
                eprintln!("loom: Codex 스레드 {thread}의 받음 확인을 쓰지 못했다: {e}");
            }
            true
        }
        Err(cause) => {
            eprintln!("loom: Codex 스레드 {thread}에 메시지를 넘기지 못했다: {cause}");
            if let Err(e) = port.release(&ids) {
                eprintln!("loom: Codex 스레드 {thread}의 메시지를 큐로 돌리지 못했다: {e}");
            }
            false
        }
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
