mod server;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;
use portable_pty::CommandBuilder;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, watch};

use super::adapter::{Carry, Chosen, Command, Facts, InputSeen, ResumeError, Serve};
use super::process::Served;
use super::{Change, NotTaken, ServerAdapter, Signal, Status, VersionMismatch};
use crate::accounts::{AccountId, Site};
use crate::protocol::{
    Delivery, Kind, LaunchError, MessageId, Model, ModelChoice, PermissionReply, Prompt, PromptKey,
    PromptOption, Report, ServerRecord, ServerState, Session, WaitKind,
};
use crate::screen::{Dialog, Grammar, Screen};
use crate::skills::Launch;
use server::Server;

pub const PINNED_VERSION: &str = "2.0.32";
pub const PASSWORD_ENV: &str = "OPENCODE_PASSWORD";
pub const CMD: &str = "opencode2";
const LOOM_INBOX: &str = "msg_loom";

const CONFIG_ENV: &str = "OPENCODE_CONFIG_CONTENT";

pub(super) const WAITING: &str = "When waiting for children and no independent work remains, end your turn. Their messages arrive as new input automatically. Do not run an unbounded loom wait.";

fn usage(launch: Launch) -> String {
    format!(
        "{}\n{WAITING}\n\n{}",
        super::COMMUNICATION_USAGE,
        crate::skills::always_on(launch)
    )
}

pub struct OpenCode2;

impl super::adapter::Adapter for OpenCode2 {
    fn facts(&self) -> Facts {
        Facts {
            provider: None,
            sleeps: false,
            listens_at_start: true,
            pinned: Some(CMD),
            usage_part: "usage/opencode2",
            waiting: WAITING,
            helper_shells: &[],
        }
    }

    fn leading_args(&self, _: Option<&Session>, bypass: &[String]) -> Vec<String> {
        bypass.to_vec()
    }

    fn prepare(&self, _: &Serve<'_>) -> std::io::Result<()> {
        Ok(())
    }

    fn command(&self, _: &mut CommandBuilder, _: Command<'_>) {}

    fn serve<'a>(
        &'a self,
        at: &'a Serve<'_>,
        resume: Option<&'a Session>,
        _: &'a [String],
        cmd: &'a mut CommandBuilder,
    ) -> Option<BoxFuture<'a, Result<Arc<dyn super::adapter::Server>, LaunchError>>> {
        Some(Box::pin(async move {
            let failed = |cause| LaunchError::Server {
                program: format!("{CMD} serve"),
                cause,
            };
            let node = Node::start(at, resume).await.map_err(failed)?;
            cmd.args(["--server", node.server.url(), "--session", &node.session]);
            cmd.env(PASSWORD_ENV, node.server.password());
            Ok(Arc::new(node) as Arc<dyn super::adapter::Server>)
        }))
    }

    fn reconnect<'a>(
        &'a self,
        state: &'a ServerState,
        served: Served,
        session: Option<&'a Session>,
    ) -> Option<BoxFuture<'a, Result<Arc<dyn super::adapter::Server>, String>>> {
        Some(Box::pin(async move {
            let state = State::deserialize(&state.0).map_err(|e| format!("server state: {e}"))?;
            let session = session.ok_or("OpenCode 2 세션이 없어 서버에 다시 붙지 않는다")?;
            let server = Server::connect(state.url, state.password, served);
            Ok(Arc::new(Node::follow(server, session.0.clone()))
                as Arc<dyn super::adapter::Server>)
        }))
    }

    fn choice_args(&self, _: &ModelChoice) -> Option<Vec<String>> {
        None
    }

    fn screen(&self) -> Option<(&'static [Grammar], &'static [Dialog])> {
        None
    }

    fn input_seen(&self, _: &Screen) -> Option<InputSeen> {
        None
    }

    fn install(&self, _: &Path) -> std::io::Result<()> {
        Ok(())
    }

    fn version_warning(&self, version_output: &str) -> Option<VersionMismatch> {
        let last = version_output.split_whitespace().last()?;
        let installed = last.strip_prefix('v').unwrap_or(last);
        (installed != PINNED_VERSION).then(|| VersionMismatch {
            installed: installed.to_owned(),
            pinned: PINNED_VERSION.to_owned(),
        })
    }

    fn carry(&self, _: &Session, _: &Carry<'_>) -> Result<(), ResumeError> {
        Ok(())
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelRef {
    #[serde(rename = "providerID")]
    provider_id: String,
    id: String,
    variant: Option<String>,
}

fn selected_model(event: &Value, session: &str) -> Option<Model> {
    let data = &event["data"];
    if event["type"] != "session.model.selected" || data["sessionID"] != session {
        return None;
    }
    let model = ModelRef::deserialize(&data["model"]).ok()?;
    Some(Model {
        model: Some(format!("{}/{}", model.provider_id, model.id)),
        effort: model.variant,
    })
}

#[derive(Debug, PartialEq)]
enum TurnEvent {
    Started,
    Said(String),
    Succeeded,
    Stopped,
    Enqueued { id: String },
}

fn turn_event(event: &Value, session: &str) -> Option<TurnEvent> {
    let data = &event["data"];
    if data["sessionID"] != session {
        return None;
    }
    match event["type"].as_str()? {
        "session.execution.started" => Some(TurnEvent::Started),
        "session.text.ended" => Some(TurnEvent::Said(data["text"].as_str()?.to_owned())),
        "session.execution.succeeded" => Some(TurnEvent::Succeeded),
        "session.execution.failed" | "session.execution.interrupted" => Some(TurnEvent::Stopped),
        "session.inbox.enqueued" if data["item"]["type"] == "user" => Some(TurnEvent::Enqueued {
            id: data["inboxID"].as_str()?.to_owned(),
        }),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
struct Permission {
    id: String,
    action: String,
    #[serde(default)]
    resources: Vec<String>,
}

const PERMISSION_OPTIONS: [(&str, PermissionReply); 3] = [
    ("Allow once", PermissionReply::Once),
    ("Always allow", PermissionReply::Always),
    ("Reject", PermissionReply::Reject),
];

impl Permission {
    fn summary(&self) -> String {
        match self.resources.first() {
            Some(resource) => format!("{}: {resource}", self.action),
            None => self.action.clone(),
        }
    }

    fn prompt(&self) -> Prompt {
        let body: Vec<String> = std::iter::once(self.action.clone())
            .chain(self.resources.iter().cloned())
            .collect();
        Prompt {
            rows: std::iter::once(self.id.clone())
                .chain(body.iter().cloned())
                .collect(),
            body,
            summary: self.summary(),
            options: PERMISSION_OPTIONS
                .iter()
                .map(|(label, reply)| PromptOption {
                    label: (*label).to_owned(),
                    key: PromptKey::Permission(*reply),
                })
                .collect(),
            field: None,
        }
    }
}

#[derive(Debug, PartialEq)]
struct Decision {
    request: String,
    reply: PermissionReply,
}

fn decide(asked: Option<&Permission>, chosen: &Prompt, option: usize) -> Option<Decision> {
    let asked = asked?;
    let now = asked.prompt();
    if now != *chosen {
        return None;
    }
    match now.options.get(option)?.key {
        PromptKey::Permission(reply) => Some(Decision {
            request: asked.id.clone(),
            reply,
        }),
        PromptKey::Char(_)
        | PromptKey::Tab
        | PromptKey::BackTab
        | PromptKey::Move(_)
        | PromptKey::Line(_) => None,
    }
}

fn permission_event(event: &Value, session: &str) -> bool {
    event["data"]["sessionID"] == session
        && matches!(
            event["type"].as_str(),
            Some("permission.asked" | "permission.replied")
        )
}

struct Node {
    server: Server,
    session: String,
    model: watch::Receiver<Model>,
    permission: watch::Receiver<()>,
    turns: Mutex<Option<mpsc::UnboundedReceiver<Seen>>>,
}

impl Node {
    async fn start(at: &Serve<'_>, resume: Option<&Session>) -> Result<Self, String> {
        let config = serde_json::json!({ "skills": [at.skills] }).to_string();
        let mut env: Vec<(&'static str, OsString)> = at.env.to_vec();
        env.push((CONFIG_ENV, config.into()));
        let server = Server::start(CMD, at.cwd, at.inside, &env, password()?).await?;
        let directory = at.inside.map_or(at.cwd, |inside| Path::new(inside.cwd()));
        let created = match resume {
            Some(session) => Ok(session.0.clone()),
            None => server.create_session(directory).await,
        };
        let session = match created {
            Ok(session) => session,
            Err(e) => {
                server.stop();
                return Err(e);
            }
        };
        if let Err(e) = server
            .put_instructions(&session, "loom", &usage(at.launch))
            .await
        {
            server.stop();
            return Err(e);
        }
        Ok(Self::follow(server, session))
    }

    fn follow(server: Server, session: String) -> Self {
        let (tx, model) = watch::channel(Model::default());
        let (tx_permission, permission) = watch::channel(());
        let (tx_turns, turns) = mpsc::unbounded_channel();
        let ours = session.clone();
        let mut said = None;
        server.follow_events(move |event| {
            if let Some(selected) = selected_model(&event, &ours) {
                tx.send_replace(selected);
            }
            let turn = turn_event(&event, &ours);
            if permission_event(&event, &ours) || turn == Some(TurnEvent::Stopped) {
                tx_permission.send_replace(());
            }
            match turn {
                Some(TurnEvent::Started) => {
                    said = None;
                    let _ = tx_turns.send(Seen::Started);
                }
                Some(TurnEvent::Said(text)) => said = Some(text),
                Some(TurnEvent::Succeeded) => {
                    let _ = tx_turns.send(Seen::Succeeded(said.take()));
                }
                Some(TurnEvent::Stopped) => {
                    said = None;
                    let _ = tx_turns.send(Seen::Stopped);
                }
                Some(TurnEvent::Enqueued { id }) if !id.starts_with(LOOM_INBOX) => {
                    let _ = tx_turns.send(Seen::Human);
                }
                Some(TurnEvent::Enqueued { .. }) | None => {}
            }
        });
        Self {
            server,
            session,
            model,
            permission,
            turns: Mutex::new(Some(turns)),
        }
    }

    async fn open_permission(&self) -> Result<Option<Permission>, String> {
        let reply = self.server.permissions(&self.session).await?;
        reply["data"]
            .get(0)
            .map(Permission::deserialize)
            .transpose()
            .map_err(|e| format!("permission list: {e}"))
    }

    async fn permission_wait(&self) -> Option<(WaitKind, String, Option<Prompt>)> {
        match self.open_permission().await {
            Ok(now) => now.map(|p| (WaitKind::Approval, p.summary(), Some(p.prompt()))),
            Err(e) => {
                eprintln!("loom: OpenCode 2 승인 요청을 읽지 못했다: {e}");
                let text = format!(
                    "Could not read the permission request: {e}. Answer it in the node terminal."
                );
                Some((WaitKind::Approval, text, None))
            }
        }
    }

    async fn interrupt(&self) -> Result<(), String> {
        self.server.interrupt(&self.session).await
    }

    async fn prompt(&self, mail: &[MessageId], text: &str, inbox: Inbox) -> Result<(), String> {
        let id = inbox_id(&self.session, mail)?;
        self.server
            .prompt(&self.session, &id, text, inbox.as_str())
            .await
    }
}

#[derive(Serialize, Deserialize)]
struct State {
    url: String,
    password: String,
}

impl super::adapter::Server for Node {
    fn record(&self) -> ServerRecord {
        let state = State {
            url: self.server.url().to_owned(),
            password: self.server.password().to_owned(),
        };
        ServerRecord {
            pid: self.server.process().pid,
            state: ServerState(serde_json::to_value(state).expect("문자열 표는 늘 직렬화된다")),
        }
    }

    fn session(&self) -> Option<Session> {
        Some(Session(self.session.clone()))
    }

    fn stop(&self) {
        self.server.stop();
    }

    fn resume(&self) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }

    fn session_file<'a>(&'a self, _: &'a Session) -> BoxFuture<'a, Option<String>> {
        Box::pin(async { None })
    }

    fn choose<'a>(
        &'a self,
        prompt: &'a Prompt,
        option: usize,
    ) -> Option<BoxFuture<'a, Result<Chosen, String>>> {
        Some(Box::pin(async move {
            let asked = self.open_permission().await?;
            let Some(decision) = decide(asked.as_ref(), prompt, option) else {
                return Ok(Chosen::PromptChanged);
            };
            self.server
                .reply_permission(&self.session, &decision.request, decision.reply)
                .await?;
            Ok(Chosen::Sent)
        }))
    }

    fn link(self: Arc<Self>) -> Box<dyn ServerAdapter> {
        let turns = self.turns.lock().unwrap().take();
        let (read, reads) = mpsc::unbounded_channel();
        Box::new(Adapter {
            turns,
            stopped: self.server.stopped(),
            model: self.model.clone(),
            permission: self.permission.clone(),
            node: self,
            pending: Some(Seen::Connected),
            model_moved: false,
            asked: true,
            ended: false,
            read,
            reads,
            sent: 0,
            answered: 0,
            read_wait: None,
        })
    }
}

fn password() -> Result<String, String> {
    let half = || getrandom::u64().map_err(|e| format!("password: {e}"));
    Ok(format!("{:016x}{:016x}", half()?, half()?))
}

enum Seen {
    Connected,
    Started,
    Succeeded(Option<String>),
    Stopped,
    Human,
}

struct Adapter {
    node: Arc<Node>,
    turns: Option<mpsc::UnboundedReceiver<Seen>>,
    stopped: watch::Receiver<bool>,
    model: watch::Receiver<Model>,
    permission: watch::Receiver<()>,
    pending: Option<Seen>,
    model_moved: bool,
    asked: bool,
    ended: bool,
    read: mpsc::UnboundedSender<(u64, PermissionWait)>,
    reads: mpsc::UnboundedReceiver<(u64, PermissionWait)>,
    sent: u64,
    answered: u64,
    read_wait: Option<PermissionWait>,
}

type PermissionWait = Option<(WaitKind, String, Option<Prompt>)>;

impl ServerAdapter for Adapter {
    fn takes(&self, _: Status) -> &'static [&'static [Kind]] {
        match self.ended {
            true => &[],
            false => &[&[Kind::Steer, Kind::Interrupt], &[Kind::Queue]],
        }
    }

    fn status(&mut self) -> Option<Status> {
        match self.pending.as_ref()? {
            Seen::Started => Some(Status::Busy),
            Seen::Connected | Seen::Succeeded(_) | Seen::Stopped => Some(Status::Idle),
            Seen::Human => None,
        }
    }

    fn signals(&mut self, _: Option<Change>) -> BoxFuture<'_, Vec<Signal>> {
        Box::pin(async move {
            let mut signals = Vec::new();
            if std::mem::take(&mut self.model_moved) {
                let model = self.model.borrow_and_update().clone();
                signals.push(Signal::Report(Report::Model(model)));
            }
            if std::mem::take(&mut self.asked) {
                self.permission.borrow_and_update();
                self.sent += 1;
                let (node, read, sent) = (self.node.clone(), self.read.clone(), self.sent);
                tokio::spawn(async move { drop(read.send((sent, node.permission_wait().await))) });
            }
            if let Some(wait) = self.read_wait.take() {
                signals.push(Signal::Report(Report::HumanWait(wait)));
            }
            let turn = match self.pending.take() {
                None | Some(Seen::Connected | Seen::Started) => None,
                Some(Seen::Succeeded(reply)) => Some(Signal::TurnEnded(reply)),
                Some(Seen::Stopped) => Some(Signal::Stopped),
                Some(Seen::Human) => Some(Signal::HumanInput),
            };
            signals.extend(turn);
            signals
        })
    }

    fn changed(&mut self) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            if self.ended {
                return false;
            }
            let Self {
                turns,
                stopped,
                model,
                permission,
                pending,
                model_moved,
                asked,
                reads,
                answered,
                read_wait,
                ..
            } = self;
            let turn = async {
                match turns.as_mut() {
                    Some(turns) => turns.recv().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                _ = stopped.wait_for(|s| *s) => false,
                turn = turn => {
                    match turn {
                        Some(turn) => *pending = Some(turn),
                        None => *turns = None,
                    }
                    true
                }
                moved = model.changed() => {
                    *model_moved = moved.is_ok();
                    moved.is_ok()
                }
                changed = permission.changed() => {
                    *asked = changed.is_ok();
                    changed.is_ok()
                }
                Some((sent, wait)) = reads.recv() => {
                    if sent > *answered {
                        *answered = sent;
                        *read_wait = Some(wait);
                    }
                    true
                }
            }
        })
    }

    fn inject<'a>(
        &'a mut self,
        taken: &'a [Delivery],
        _: Status,
    ) -> BoxFuture<'a, Result<(), NotTaken>> {
        Box::pin(async move {
            deliver(&self.node, taken)
                .await
                .map_err(|e| NotTaken::ServerStopped(self.refused(e)))
        })
    }

    fn stop_shells(&self) -> BoxFuture<'_, ()> {
        Box::pin(self.node.server.stop_shells(&self.node.session))
    }

    fn start_turn<'a>(&'a mut self, text: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let sent = self.node.prompt(&[], text, Inbox::Queue).await;
            sent.map_err(|e| self.refused(e))
        })
    }

    fn steer<'a>(&'a mut self, text: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let sent = self.node.prompt(&[], text, Inbox::Steer).await;
            sent.map_err(|e| self.refused(e))
        })
    }
}

impl Adapter {
    fn refused(&mut self, cause: String) -> String {
        self.node.server.stop();
        self.ended = true;
        format!("{cause}; loom stopped the OpenCode 2 server")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Inbox {
    Steer,
    Queue,
}

impl Inbox {
    fn as_str(self) -> &'static str {
        match self {
            Inbox::Steer => "steer",
            Inbox::Queue => "queue",
        }
    }
}

async fn deliver(api: &Node, taken: &[Delivery]) -> Result<(), String> {
    if taken.iter().any(|d| d.kind == Kind::Interrupt)
        && let Err(e) = api.interrupt().await
    {
        eprintln!("loom: OpenCode 2 실행을 멈추지 못했다: {e}");
    }
    let inbox = match taken.iter().all(|d| d.kind == Kind::Queue) {
        true => Inbox::Queue,
        false => Inbox::Steer,
    };
    let mail: Vec<MessageId> = taken.iter().map(|d| d.id).collect();
    api.prompt(&mail, &super::message_text(taken), inbox).await
}

fn inbox_id(session: &str, mail: &[MessageId]) -> Result<String, String> {
    let session = session.strip_prefix("ses_").unwrap_or(session);
    let input = match mail.is_empty() {
        true => format!(
            "n{:016x}",
            getrandom::u64().map_err(|e| format!("inbox id: {e}"))?
        ),
        false => mail
            .iter()
            .map(|id| id.0.to_string())
            .collect::<Vec<_>>()
            .join("_"),
    };
    Ok(format!("{LOOM_INBOX}_{session}_{input}"))
}
