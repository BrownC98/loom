mod server;

use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use portable_pty::CommandBuilder;
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
    Prompt, PromptKey, PromptOption, Refusal, Report, Session, TakeRequest, WaitKind,
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
            let failed = |cause| LaunchError::Helper {
                program: format!("{CMD} serve"),
                cause,
            };
            let rules = permission_rules(bypass);
            let (node, turns) = Node::start(at, resume, &rules).await.map_err(failed)?;
            cmd.args(["--server", node.server.url(), "--session", &node.session]);
            cmd.env(PASSWORD_ENV, node.server.password());
            let record = node.server.process().record(State {
                url: node.server.url().to_owned(),
                password: node.server.password().to_owned(),
            });
            let session = Session(node.session.clone());
            tokio::spawn(drive(Arc::new(node), turns, port));
            Ok(Started {
                record: Some(record),
                session: Some(session),
                before_start: None,
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
            let Some(session) = &rec.session else {
                served.stop();
                return Err("OpenCode 2 세션이 없어 서버에 다시 붙지 않는다".into());
            };
            let server = Server::connect(state.url, state.password, served);
            let (node, turns) = Node::follow(server, session.0.clone());
            tokio::spawn(drive(Arc::new(node), turns, port));
            Ok(())
        })
    }

    fn answer<'a>(
        &'a self,
        rec: &'a Record,
        prompt: &'a Prompt,
        option: usize,
    ) -> BoxFuture<'a, Result<Chosen, String>> {
        Box::pin(async move {
            let (Some((served, state)), Some(session)) =
                (Served::stored::<State>(rec), &rec.session)
            else {
                return Ok(Chosen::OnScreen);
            };
            let server = Server::connect(state.url, state.password, served);
            let asked = open_permission(&server, &session.0).await?;
            let Some(decision) = decide(asked.as_ref(), prompt, option) else {
                return Ok(Chosen::PromptChanged);
            };
            server
                .reply_permission(&session.0, &decision.request, decision.reply)
                .await?;
            Ok(Chosen::Sent)
        })
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

    fn end_detached<'a>(&'a self, _: &'a Record, _: &'a Site) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }

    fn record<'a>(&'a self, _: &'a Record) -> BoxFuture<'a, Option<String>> {
        Box::pin(async { None })
    }

    fn shell_root(&self, rec: &Record) -> Option<u32> {
        Served::stored::<State>(rec)?.0.here()
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

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
enum PermissionReply {
    Once,
    Always,
    Reject,
}

impl PermissionReply {
    fn key(self) -> PromptKey {
        let text = serde_json::to_value(self).expect("enum은 늘 직렬화된다");
        PromptKey::Reply(text.as_str().unwrap_or_default().to_owned())
    }
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
                    key: reply.key(),
                    writes: false,
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
    let (_, reply) = PERMISSION_OPTIONS.get(option)?;
    Some(Decision {
        request: asked.id.clone(),
        reply: *reply,
    })
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
}

impl Node {
    async fn start(
        at: &LaunchAt<'_>,
        resume: Option<&Session>,
        rules: &[Value],
    ) -> Result<(Self, mpsc::UnboundedReceiver<Seen>), String> {
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
        let set = async {
            server.set_permissions(&session, rules).await?;
            server
                .put_instructions(&session, "loom", &usage(at.launch))
                .await
        };
        if let Err(e) = set.await {
            server.stop();
            return Err(e);
        }
        Ok(Self::follow(server, session))
    }

    fn follow(server: Server, session: String) -> (Self, mpsc::UnboundedReceiver<Seen>) {
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
        let node = Self {
            server,
            session,
            model,
            permission,
        };
        (node, turns)
    }

    async fn permission_wait(&self) -> PermissionWait {
        match open_permission(&self.server, &self.session).await {
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

    async fn prompt(&self, mail: &[MessageId], text: &str, inbox: Inbox) -> Result<(), String> {
        let id = inbox_id(&self.session, mail)?;
        self.server
            .prompt(&self.session, &id, text, inbox.as_str())
            .await
    }
}

fn permission_rules(bypass: &[String]) -> Vec<Value> {
    match bypass.is_empty() {
        true => Vec::new(),
        false => vec![serde_json::json!({ "action": "*", "resource": "*", "effect": "allow" })],
    }
}

async fn open_permission(server: &Server, session: &str) -> Result<Option<Permission>, String> {
    let reply = server.permissions(session).await?;
    reply["data"]
        .get(0)
        .map(Permission::deserialize)
        .transpose()
        .map_err(|e| format!("permission list: {e}"))
}

#[derive(Serialize, Deserialize)]
struct State {
    url: String,
    password: String,
}

fn password() -> Result<String, String> {
    let half = || getrandom::u64().map_err(|e| format!("password: {e}"));
    Ok(format!("{:016x}{:016x}", half()?, half()?))
}

enum Seen {
    Started,
    Succeeded(Option<String>),
    Stopped,
    Human,
}

type PermissionWait = Option<(WaitKind, String, Option<Prompt>)>;

const BUSY_KINDS: &[Kind] = &[Kind::Steer, Kind::Interrupt];
const HUMAN_STEP: &str = "human";
const TURN_STEP: &str = "started";

async fn pending<T>(future: &mut Option<BoxFuture<'_, T>>) -> T {
    match future {
        Some(future) => future.await,
        None => std::future::pending().await,
    }
}

fn take_request(step: &str, human_input: bool) -> TakeRequest {
    TakeRequest {
        step: step.to_owned(),
        kinds: Vec::new(),
        limit_bytes: 0,
        session: None,
        human_input,
        prompt: None,
        turn: None,
    }
}

async fn drive(node: Arc<Node>, mut turns: mpsc::UnboundedReceiver<Seen>, port: Arc<dyn Port>) {
    let mut stopped = node.server.stopped();
    let mut model = node.model.clone();
    let mut permission = node.permission.clone();
    let (read, mut reads) = mpsc::unbounded_channel::<(u64, PermissionWait)>();
    let (mut sent, mut answered) = (0, 0);
    let mut idle: Option<BoxFuture<'_, Result<IdleReply, String>>> = None;
    let mut busy: Option<BoxFuture<'_, Result<Vec<Delivery>, Refusal>>> = None;
    let mut ended = port.ended();
    let mut asked = true;
    let mut seen = Some(Seen::Stopped);
    let mut turn_end = None;
    let mut rest = false;
    loop {
        if std::mem::take(&mut asked) {
            permission.borrow_and_update();
            sent += 1;
            let (node, read) = (node.clone(), read.clone());
            tokio::spawn(async move { drop(read.send((sent, node.permission_wait().await))) });
        }
        let (mut held, mut note) = (None, None);
        match seen.take() {
            Some(Seen::Started) => {
                if idle.is_some() && port.take(take_request(TURN_STEP, false)).is_ok() {
                    held = settle(&mut idle);
                }
                port.report(Report::Busy);
                busy.get_or_insert_with(|| port.next_mail(BUSY_KINDS));
            }
            Some(Seen::Human) => match port.take(take_request(HUMAN_STEP, true)) {
                Ok(taken) => {
                    held = settle(&mut idle);
                    note = taken.note;
                    rest = true;
                }
                Err(e) => eprintln!("loom: OpenCode 2 사람 입력을 처리하지 못했다: {e}"),
            },
            Some(Seen::Succeeded(reply)) => {
                busy = None;
                turn_end = Some(EndedTurn { reply, turn: None });
            }
            Some(Seen::Stopped) => busy = None,
            None => {}
        }
        if let Some(reply) = held
            && let Err((ids, e)) = answer(&node, &*port, reply).await
        {
            refused(&node, &*port, ids, e);
            (&mut ended).await;
            break;
        }
        if let Some(note) = note
            && let Err(e) = node.prompt(&[], &note, Inbox::Steer).await
        {
            refused(&node, &*port, Vec::new(), e);
            (&mut ended).await;
            break;
        }
        if busy.is_none() && idle.is_none() && !rest {
            let turn_end = turn_end.take();
            idle = Some(port.idle(IdleRequest {
                turn_end,
                ..IdleRequest::default()
            }));
        }
        tokio::select! {
            biased;
            () = &mut ended => break,
            () = async { drop(stopped.wait_for(|s| *s).await) } => {
                port.report(Report::HumanWait(None));
                port.report(Report::Lost);
                (&mut ended).await;
                break;
            }
            turn = turns.recv() => match turn {
                Some(turn) => {
                    seen = Some(turn);
                    rest = false;
                }
                None => {
                    port.report(Report::Lost);
                    (&mut ended).await;
                    break;
                }
            },
            Ok(()) = model.changed() => {
                let now = model.borrow_and_update().clone();
                port.report(Report::Model(now));
            }
            Ok(()) = permission.changed() => asked = true,
            Some((n, wait)) = reads.recv() => {
                if n > answered {
                    answered = n;
                    port.report(Report::HumanWait(wait));
                }
            }
            reply = pending(&mut idle) => {
                idle = None;
                let sent = match reply {
                    Ok(IdleReply::Mail(mail)) if mail.is_empty() => {
                        rest = true;
                        continue;
                    }
                    Ok(reply) => answer(&node, &*port, reply).await,
                    Err(e) => {
                        eprintln!("loom: OpenCode 2 쉬는 대기가 끝났다: {e}");
                        rest = true;
                        continue;
                    }
                };
                if let Err((ids, e)) = sent {
                    refused(&node, &*port, ids, e);
                    (&mut ended).await;
                    break;
                }
                busy = Some(port.next_mail(BUSY_KINDS));
            }
            mail = pending(&mut busy) => {
                busy = Some(port.next_mail(BUSY_KINDS));
                match mail {
                    Ok(mail) => {
                        if let Err((ids, e)) = hand(&node, &*port, &mail).await {
                            refused(&node, &*port, ids, e);
                            (&mut ended).await;
                            break;
                        }
                    }
                    Err(e) => eprintln!("loom: OpenCode 2 메일함을 읽지 못했다: {e}"),
                }
            }
        }
    }
    node.server.stop();
}

fn settle(idle: &mut Option<BoxFuture<'_, Result<IdleReply, String>>>) -> Option<IdleReply> {
    idle.take()?.now_or_never()?.ok()
}

async fn answer(
    node: &Node,
    port: &dyn Port,
    reply: IdleReply,
) -> Result<(), (Vec<MessageId>, String)> {
    match reply {
        IdleReply::Mail(mail) if mail.is_empty() => Ok(()),
        IdleReply::Mail(mail) => hand(node, port, &mail).await,
        IdleReply::Nudge { nudge } => node
            .prompt(&[], &nudge, Inbox::Queue)
            .await
            .map_err(|e| (Vec::new(), e)),
    }
}

async fn hand(
    node: &Node,
    port: &dyn Port,
    mail: &[Delivery],
) -> Result<(), (Vec<MessageId>, String)> {
    let ids: Vec<MessageId> = mail.iter().map(|d| d.id).collect();
    if mail.iter().any(|d| d.kind == Kind::Interrupt) {
        node.server.stop_shells(&node.session).await;
    }
    if let Err(e) = deliver(node, mail).await {
        return Err((ids, e));
    }
    if let Err(e) = port.ack(&ids) {
        eprintln!("loom: OpenCode 2 받음 확인을 쓰지 못했다: {e}");
    }
    Ok(())
}

fn refused(node: &Node, port: &dyn Port, ids: Vec<MessageId>, cause: String) {
    node.server.stop();
    let cause = format!("{cause}; loom stopped the OpenCode 2 server");
    eprintln!("loom: OpenCode 2에 메시지를 넘기지 못했다: {cause}");
    if !ids.is_empty() {
        port.report(Report::Refused { ids, cause });
    }
    port.report(Report::HumanWait(None));
    port.report(Report::Lost);
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
        && let Err(e) = api.server.interrupt(&api.session).await
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
