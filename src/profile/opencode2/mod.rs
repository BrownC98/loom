mod server;

use std::ffi::OsString;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;

use serde::Deserialize;
use serde_json::Value;
use tokio::sync::{mpsc, watch};

use crate::accounts::{AccountId, Site};
use crate::profile::process::Served;
use crate::protocol::{
    Delivery, Kind, MessageId, Model, PermissionReply, Prompt, PromptKey, PromptOption, Session,
};
use crate::skills::Launch;
use crate::wsl::Inside;

use portable_pty::CommandBuilder;

use super::adapter::{Carry, Command, Facts, InputSeen, NoServer, ResumeError};
use super::{Change, NotTaken, Remote, ServerAdapter, Signal, Status, VersionMismatch};
use crate::protocol::ModelChoice;
use crate::screen::{Dialog, Grammar, Screen};
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
        }
    }

    fn leading_args(&self, _: Option<&Session>, bypass: &[String]) -> Vec<String> {
        bypass.to_vec()
    }

    fn command(&self, cmd: &mut CommandBuilder, at: Command<'_>) -> Result<(), NoServer> {
        let Some(Remote::OpenCode2 {
            url,
            session,
            password,
        }) = at.remote
        else {
            return Err(NoServer);
        };
        cmd.args(["--server", url, "--session", session]);
        cmd.env(PASSWORD_ENV, password);
        Ok(())
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
pub struct Permission {
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
    pub fn summary(&self) -> String {
        match self.resources.first() {
            Some(resource) => format!("{}: {resource}", self.action),
            None => self.action.clone(),
        }
    }

    pub fn prompt(&self) -> Prompt {
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
pub struct Decision {
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

pub struct Node {
    server: Server,
    session: String,
    model: watch::Receiver<Model>,
    permission: watch::Receiver<()>,
    turns: Mutex<Option<mpsc::UnboundedReceiver<Seen>>>,
}

impl Node {
    pub async fn start(
        cwd: &Path,
        inside: Option<&Inside>,
        node_env: &[(&'static str, OsString)],
        skills: &Path,
        password: String,
        launch: Launch,
        resume: Option<&Session>,
    ) -> Result<Self, String> {
        let config = serde_json::json!({ "skills": [skills] }).to_string();
        let mut env = node_env.to_vec();
        env.push((CONFIG_ENV, config.into()));
        let server = Server::start(CMD, cwd, inside, &env, password).await?;
        let directory = inside.map_or(cwd, |inside| Path::new(inside.cwd()));
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
            .put_instructions(&session, "loom", &usage(launch))
            .await
        {
            server.stop();
            return Err(e);
        }
        Ok(Self::follow(server, session))
    }

    pub fn reconnect(url: String, process: Served, session: Session, password: String) -> Self {
        Self::follow(Server::connect(url, password, process), session.0)
    }

    pub fn link(&self) -> crate::protocol::ServerLink {
        crate::protocol::ServerLink::OpenCode2 {
            url: self.server.url().to_owned(),
            pid: self.server.process().pid,
            password: self.server.password().to_owned(),
        }
    }

    pub fn session(&self) -> Session {
        Session(self.session.clone())
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

    pub fn model(&self) -> watch::Receiver<Model> {
        self.model.clone()
    }

    pub fn permission_changed(&self) -> watch::Receiver<()> {
        self.permission.clone()
    }

    pub async fn open_permission(&self) -> Result<Option<Permission>, String> {
        let reply = self.server.permissions(&self.session).await?;
        reply["data"]
            .get(0)
            .map(Permission::deserialize)
            .transpose()
            .map_err(|e| format!("permission list: {e}"))
    }

    pub async fn decide(&self, chosen: &Prompt, option: usize) -> Result<Option<Decision>, String> {
        Ok(decide(
            self.open_permission().await?.as_ref(),
            chosen,
            option,
        ))
    }

    pub async fn reply(&self, decision: Decision) -> Result<(), String> {
        self.server
            .reply_permission(&self.session, &decision.request, decision.reply)
            .await
    }

    pub fn remote(&self) -> super::Remote<'_> {
        super::Remote::OpenCode2 {
            url: self.server.url(),
            session: &self.session,
            password: self.server.password(),
        }
    }

    pub fn stop(&self) {
        self.server.stop();
    }

    pub fn adapter(self: &Arc<Self>) -> impl ServerAdapter + use<> {
        Adapter {
            turns: self.turns.lock().unwrap().take(),
            stopped: self.server.stopped(),
            api: self.clone(),
            pending: Some(Seen::Connected),
            ended: false,
        }
    }
}

enum Seen {
    Connected,
    Started,
    Succeeded(Option<String>),
    Stopped,
    Human,
}

struct Adapter<A> {
    api: Arc<A>,
    turns: Option<mpsc::UnboundedReceiver<Seen>>,
    stopped: watch::Receiver<bool>,
    pending: Option<Seen>,
    ended: bool,
}

impl<A: Api> ServerAdapter for Adapter<A> {
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

    async fn signal(&mut self, _: Option<Change>) -> Option<Signal> {
        match self.pending.take()? {
            Seen::Connected | Seen::Started => None,
            Seen::Succeeded(reply) => Some(Signal::TurnEnded(reply)),
            Seen::Stopped => Some(Signal::Stopped),
            Seen::Human => Some(Signal::HumanInput),
        }
    }

    async fn changed(&mut self) -> bool {
        if self.ended {
            return false;
        }
        let Self {
            turns,
            stopped,
            pending,
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
        }
    }

    async fn inject(&mut self, taken: &[Delivery], _: Status) -> Result<(), NotTaken> {
        deliver(&*self.api, taken)
            .await
            .map_err(|e| NotTaken::ServerStopped(self.refused(e)))
    }

    async fn stop_shells(&self) {
        self.api.stop_shells().await;
    }

    async fn start_turn(&mut self, text: &str) -> Result<(), String> {
        let sent = self.api.prompt(&[], text, Inbox::Queue).await;
        sent.map_err(|e| self.refused(e))
    }

    async fn steer(&mut self, text: &str) -> Result<(), String> {
        let sent = self.api.prompt(&[], text, Inbox::Steer).await;
        sent.map_err(|e| self.refused(e))
    }
}

impl<A: Api> Adapter<A> {
    fn refused(&mut self, cause: String) -> String {
        self.api.stop_server();
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

trait Api: Send + Sync {
    fn interrupt(&self) -> impl Future<Output = Result<(), String>> + Send;
    fn prompt(
        &self,
        mail: &[MessageId],
        text: &str,
        inbox: Inbox,
    ) -> impl Future<Output = Result<(), String>> + Send;
    fn stop_shells(&self) -> impl Future<Output = ()> + Send;
    fn stop_server(&self);
}

impl Api for Node {
    async fn interrupt(&self) -> Result<(), String> {
        self.server.interrupt(&self.session).await
    }

    async fn prompt(&self, mail: &[MessageId], text: &str, inbox: Inbox) -> Result<(), String> {
        let id = inbox_id(&self.session, mail)?;
        self.server
            .prompt(&self.session, &id, text, inbox.as_str())
            .await
    }

    async fn stop_shells(&self) {
        self.server.stop_shells(&self.session).await;
    }

    fn stop_server(&self) {
        self.server.stop();
    }
}

async fn deliver(api: &impl Api, taken: &[Delivery]) -> Result<(), String> {
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
