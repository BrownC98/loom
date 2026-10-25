mod server;

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::Value;
use tokio::sync::{mpsc, watch};

use crate::protocol::{Delivery, Kind, Model, PermissionReply, Prompt, PromptKey, PromptOption};
use crate::skills::Launch;
use crate::wsl::Inside;

use super::{Mailbox, VersionMismatch};
use server::Server;

pub const PINNED_VERSION: &str = "2.0.32";
pub const PASSWORD_ENV: &str = "OPENCODE_PASSWORD";
pub const CMD: &str = "opencode2";
const PROMPT_ATTEMPTS: u32 = 5;
const PROMPT_RETRY_DELAY: Duration = Duration::from_secs(1);

const CONFIG_ENV: &str = "OPENCODE_CONFIG_CONTENT";

pub(super) const WAITING: &str = "When waiting for children and no independent work remains, end your turn. Their messages arrive as new input automatically. Do not run an unbounded loom wait.";

fn usage(launch: Launch) -> String {
    format!(
        "{}\n{WAITING}\n\n{}",
        super::COMMUNICATION_USAGE,
        crate::skills::always_on(launch)
    )
}

pub fn args(url: &str, session: &str) -> Vec<String> {
    vec![
        "--server".into(),
        url.into(),
        "--session".into(),
        session.into(),
    ]
}

pub fn version_warning(version_output: &str) -> Option<VersionMismatch> {
    let last = version_output.split_whitespace().last()?;
    let installed = last.strip_prefix('v').unwrap_or(last);
    (installed != PINNED_VERSION).then(|| VersionMismatch {
        installed: installed.to_owned(),
        pinned: PINNED_VERSION.to_owned(),
    })
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

enum Turn {
    Ended(Option<String>),
    HumanInput,
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

#[derive(Clone, Debug, PartialEq, Deserialize)]
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

#[derive(Debug, PartialEq)]
enum PermissionEvent {
    Asked(Permission),
    Replied { request: String },
}

fn permission_event(event: &Value, session: &str) -> Option<PermissionEvent> {
    let data = &event["data"];
    if data["sessionID"] != session {
        return None;
    }
    match event["type"].as_str()? {
        "permission.asked" => Permission::deserialize(data)
            .ok()
            .map(PermissionEvent::Asked),
        "permission.replied" => Some(PermissionEvent::Replied {
            request: data["requestID"].as_str()?.to_owned(),
        }),
        _ => None,
    }
}

pub struct Node {
    server: Server,
    session: String,
    model: watch::Receiver<Model>,
    working: watch::Receiver<bool>,
    permission: watch::Receiver<Option<Permission>>,
    turns: Mutex<Option<mpsc::UnboundedReceiver<Turn>>>,
    sent: Arc<Mutex<HashSet<String>>>,
}

impl Node {
    pub async fn start(
        cwd: &Path,
        inside: Option<&Inside>,
        node_env: &[(&'static str, OsString)],
        skills: &Path,
        password: String,
        launch: Launch,
        resume: Option<&str>,
    ) -> Result<Self, String> {
        let config = serde_json::json!({ "skills": [skills] }).to_string();
        let mut env = node_env.to_vec();
        env.push((CONFIG_ENV, config.into()));
        let server = Server::start(CMD, cwd, inside, &env, password).await?;
        let directory = inside.map_or(cwd, |inside| Path::new(inside.cwd()));
        let created = match resume {
            Some(session) => Ok(session.to_owned()),
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

    pub fn reconnect(url: String, pid: Option<u32>, session: String, password: String) -> Self {
        Self::follow(Server::connect(url, password, pid), session)
    }

    pub fn link(&self) -> crate::protocol::ServerLink {
        crate::protocol::ServerLink::OpenCode2 {
            url: self.server.url().to_owned(),
            pid: self.server.pid(),
            session: self.session.clone(),
            password: self.server.password().to_owned(),
        }
    }

    fn follow(server: Server, session: String) -> Self {
        let (tx, model) = watch::channel(Model::default());
        let (tx_working, working) = watch::channel(false);
        let (tx_permission, permission) = watch::channel(None);
        let (tx_turns, turns) = mpsc::unbounded_channel();
        let sent = Arc::new(Mutex::new(HashSet::new()));
        let ours = session.clone();
        let ours_sent = sent.clone();
        let mut said = None;
        let mut asked: Vec<Permission> = Vec::new();
        server.follow_events(move |event| {
            if let Some(selected) = selected_model(&event, &ours) {
                tx.send_replace(selected);
            }
            match permission_event(&event, &ours) {
                Some(PermissionEvent::Asked(p)) if !asked.iter().any(|a| a.id == p.id) => {
                    asked.push(p)
                }
                Some(PermissionEvent::Replied { request }) => asked.retain(|a| a.id != request),
                Some(PermissionEvent::Asked(_)) | None => {}
            }
            let turn = turn_event(&event, &ours);
            if turn == Some(TurnEvent::Stopped) {
                asked.clear();
            }
            tx_permission.send_if_modified(|now| {
                let next = asked.first();
                let changed = now.as_ref() != next;
                if changed {
                    *now = next.cloned();
                }
                changed
            });
            match turn {
                Some(TurnEvent::Started) => {
                    said = None;
                    tx_working.send_replace(true);
                }
                Some(TurnEvent::Said(text)) => said = Some(text),
                Some(TurnEvent::Succeeded) => {
                    tx_working.send_replace(false);
                    let _ = tx_turns.send(Turn::Ended(said.take()));
                }
                Some(TurnEvent::Stopped) => {
                    tx_working.send_replace(false);
                }
                Some(TurnEvent::Enqueued { id }) if !ours_sent.lock().unwrap().remove(&id) => {
                    let _ = tx_turns.send(Turn::HumanInput);
                }
                Some(TurnEvent::Enqueued { .. }) | None => {}
            }
        });
        Self {
            server,
            session,
            model,
            working,
            permission,
            turns: Mutex::new(Some(turns)),
            sent,
        }
    }

    pub fn model(&self) -> watch::Receiver<Model> {
        self.model.clone()
    }

    pub fn working(&self) -> watch::Receiver<bool> {
        self.working.clone()
    }

    pub fn permission(&self) -> watch::Receiver<Option<Permission>> {
        self.permission.clone()
    }

    pub fn decide(&self, chosen: &Prompt, option: usize) -> Option<Decision> {
        decide(self.permission.borrow().as_ref(), chosen, option)
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

    pub async fn stop_shells(&self) {
        self.server.stop_shells(&self.session).await;
    }

    pub fn stop(&self) {
        self.server.stop();
    }

    pub async fn run(&self, mailbox: impl Mailbox) {
        let api = Session {
            server: &self.server,
            id: &self.session,
            sent: &self.sent,
        };
        let mut stopped = self.server.stopped();
        let mut turns = self.turns.lock().unwrap().take();
        loop {
            let changed = mailbox.changed();
            match next(&mailbox) {
                Ok(Some(batch)) => {
                    if batch.interrupt {
                        self.stop_shells().await;
                    }
                    match deliver(&api, &batch, PROMPT_RETRY_DELAY).await {
                        Ok(()) => {
                            if let Err(e) = mailbox.ack(&batch.taken) {
                                eprintln!("loom: OpenCode 2 노드의 받음 확인을 쓰지 못했다: {e}");
                            }
                            continue;
                        }
                        Err(e) => {
                            eprintln!(
                                "loom: OpenCode 2 세션 {}에 메시지를 넘기지 못했다: {e}",
                                self.session
                            );
                            if let Err(e) = mailbox.release(&batch.taken) {
                                eprintln!(
                                    "loom: OpenCode 2 노드의 메시지를 큐로 돌리지 못했다: {e}"
                                );
                            }
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => eprintln!("loom: OpenCode 2 노드의 메일함을 읽지 못했다: {e}"),
            }
            let turn = tokio::select! {
                _ = changed => continue,
                _ = stopped.wait_for(|s| *s) => return,
                Some(turn) = async { turns.as_mut()?.recv().await } => turn,
            };
            let (text, inbox) = match turn {
                Turn::Ended(reply) => (mailbox.turn_ended(reply), Inbox::Queue),
                Turn::HumanInput => (mailbox.human_input(), Inbox::Steer),
            };
            if let Some(text) = text
                && let Err(e) = put(&api, &text, inbox, PROMPT_RETRY_DELAY).await
            {
                eprintln!(
                    "loom: OpenCode 2 세션 {}을 다시 일하게 하지 못했다: {e}",
                    self.session
                );
            }
        }
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

#[derive(Debug, PartialEq)]
struct Batch {
    taken: Vec<Delivery>,
    inbox: Inbox,
    interrupt: bool,
}

fn next(mailbox: &impl Mailbox) -> Result<Option<Batch>, String> {
    let urgent = mailbox.take(&[Kind::Steer, Kind::Interrupt], super::BATCH_LIMIT_BYTES)?;
    if !urgent.is_empty() {
        let interrupt = urgent.iter().any(|d| d.kind == Kind::Interrupt);
        return Ok(Some(Batch {
            taken: urgent,
            inbox: Inbox::Steer,
            interrupt,
        }));
    }
    let queued = mailbox.take(&[Kind::Queue], super::BATCH_LIMIT_BYTES)?;
    Ok((!queued.is_empty()).then_some(Batch {
        taken: queued,
        inbox: Inbox::Queue,
        interrupt: false,
    }))
}

trait Api {
    async fn interrupt(&self) -> Result<(), String>;
    async fn prompt(&self, id: &str, text: &str, inbox: Inbox) -> Result<(), String>;
}

struct Session<'a> {
    server: &'a Server,
    id: &'a str,
    sent: &'a Mutex<HashSet<String>>,
}

impl Api for Session<'_> {
    async fn interrupt(&self) -> Result<(), String> {
        self.server.interrupt(self.id).await
    }

    async fn prompt(&self, id: &str, text: &str, inbox: Inbox) -> Result<(), String> {
        self.sent.lock().unwrap().insert(id.to_owned());
        self.server.prompt(self.id, id, text, inbox.as_str()).await
    }
}

async fn deliver(api: &impl Api, batch: &Batch, retry_delay: Duration) -> Result<(), String> {
    if batch.interrupt
        && let Err(e) = api.interrupt().await
    {
        eprintln!("loom: OpenCode 2 실행을 멈추지 못했다: {e}");
    }
    put(
        api,
        &super::message_text(&batch.taken),
        batch.inbox,
        retry_delay,
    )
    .await
}

async fn put(
    api: &impl Api,
    text: &str,
    inbox: Inbox,
    retry_delay: Duration,
) -> Result<(), String> {
    let id = message_id();
    let mut attempt = 1;
    loop {
        match api.prompt(&id, text, inbox).await {
            Ok(()) => return Ok(()),
            Err(e) if attempt == PROMPT_ATTEMPTS => return Err(e),
            Err(_) => {
                attempt += 1;
                tokio::time::sleep(retry_delay).await;
            }
        }
    }
}

fn message_id() -> String {
    static LAST: AtomicU64 = AtomicU64::new(0);
    const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let now = ms * 0x1000;
    let next = |last: u64| now.max(last + 1);
    let last = LAST
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |last| {
            Some(next(last))
        })
        .expect("늘 Some을 돌려준다");
    let time = next(last) & 0xFFFF_FFFF_FFFF;
    let mut random = [0u8; 14];
    let _ = getrandom::fill(&mut random);
    let tail: String = random
        .iter()
        .map(|b| BASE62[*b as usize % BASE62.len()] as char)
        .collect();
    format!("msg_{time:012x}{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_text_has_the_ask_rules() {
        super::super::assert_every_launch_asks(|launch| (usage(launch), usage(launch)));
    }
    use serde_json::json;
    use std::sync::Mutex;

    fn delivery(kind: Kind, body: &str) -> Delivery {
        Delivery {
            id: crate::protocol::MessageId(1),
            from: "부모".into(),
            kind,
            body: body.into(),
            what: Some(crate::protocol::MessageKind::Note),
        }
    }

    #[derive(Default)]
    struct FakeMailbox {
        queued: Mutex<Vec<Delivery>>,
        takes: Mutex<Vec<(Vec<Kind>, usize)>>,
    }

    impl Mailbox for FakeMailbox {
        fn changed(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            Box::pin(std::future::pending())
        }

        fn take(&self, kinds: &[Kind], limit_bytes: usize) -> Result<Vec<Delivery>, String> {
            self.takes
                .lock()
                .unwrap()
                .push((kinds.to_vec(), limit_bytes));
            let mut queued = self.queued.lock().unwrap();
            let (taken, kept) = queued.drain(..).partition(|d| kinds.contains(&d.kind));
            *queued = kept;
            Ok(taken)
        }

        fn ack(&self, _taken: &[Delivery]) -> Result<(), String> {
            Ok(())
        }

        fn release(&self, _taken: &[Delivery]) -> Result<(), String> {
            Ok(())
        }

        fn turn_ended(&self, _reply: Option<String>) -> Option<String> {
            None
        }

        fn human_input(&self) -> Option<String> {
            None
        }
    }

    #[test]
    fn turn_events_come_from_this_session_only() {
        let event = |kind: &str, data: Value| json!({ "type": kind, "data": data });
        let ours = |extra: Value| {
            let mut data = json!({ "sessionID": "s" });
            data.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            data
        };
        assert_eq!(
            turn_event(&event("session.execution.started", ours(json!({}))), "s"),
            Some(TurnEvent::Started)
        );
        assert_eq!(
            turn_event(
                &event(
                    "session.text.ended",
                    ours(json!({ "text": "Shall I go?", "ordinal": 0 }))
                ),
                "s"
            ),
            Some(TurnEvent::Said("Shall I go?".into()))
        );
        assert_eq!(
            turn_event(&event("session.execution.succeeded", ours(json!({}))), "s"),
            Some(TurnEvent::Succeeded)
        );
        for stopped in ["session.execution.failed", "session.execution.interrupted"] {
            assert_eq!(
                turn_event(&event(stopped, ours(json!({}))), "s"),
                Some(TurnEvent::Stopped)
            );
        }
        assert_eq!(
            turn_event(
                &event("session.execution.succeeded", ours(json!({}))),
                "other"
            ),
            None
        );
        assert_eq!(
            turn_event(
                &event("session.text.delta", ours(json!({ "delta": "x" }))),
                "s"
            ),
            None
        );
        let enqueued = |kind: &str| {
            ours(
                json!({ "inboxID": "msg_1", "item": { "type": kind, "payload": { "text": "hi" } } }),
            )
        };
        assert_eq!(
            turn_event(&event("session.inbox.enqueued", enqueued("user")), "s"),
            Some(TurnEvent::Enqueued { id: "msg_1".into() })
        );
        assert_eq!(
            turn_event(&event("session.inbox.enqueued", enqueued("other")), "s"),
            None
        );
    }

    #[derive(Default)]
    struct FakeApi {
        fail: u32,
        calls: Mutex<Vec<String>>,
    }

    impl Api for FakeApi {
        async fn interrupt(&self) -> Result<(), String> {
            self.calls.lock().unwrap().push("interrupt".into());
            Ok(())
        }

        async fn prompt(&self, id: &str, text: &str, inbox: Inbox) -> Result<(), String> {
            let mut calls = self.calls.lock().unwrap();
            calls.push(format!("{} {id} {text}", inbox.as_str()));
            if calls.len() as u32 <= self.fail {
                return Err("연결이 끊겼다".into());
            }
            Ok(())
        }
    }

    #[test]
    fn steer_and_interrupt_go_before_the_queue_and_share_the_batch_limit() {
        let mailbox = FakeMailbox::default();
        *mailbox.queued.lock().unwrap() = vec![
            delivery(Kind::Queue, "a"),
            delivery(Kind::Steer, "b"),
            delivery(Kind::Interrupt, "c"),
        ];
        let first = next(&mailbox).unwrap().unwrap();
        assert_eq!(first.inbox, Inbox::Steer);
        assert!(first.interrupt);
        assert_eq!(
            first.taken,
            [delivery(Kind::Steer, "b"), delivery(Kind::Interrupt, "c")]
        );
        let second = next(&mailbox).unwrap().unwrap();
        assert_eq!(second.inbox, Inbox::Queue);
        assert!(!second.interrupt);
        assert_eq!(second.taken, [delivery(Kind::Queue, "a")]);
        assert_eq!(next(&mailbox).unwrap(), None);
        let limit = crate::profile::BATCH_LIMIT_BYTES;
        assert!(
            mailbox
                .takes
                .lock()
                .unwrap()
                .iter()
                .all(|(_, l)| *l == limit)
        );
    }

    #[tokio::test]
    async fn interrupt_stops_the_run_then_steers_the_body() {
        let api = FakeApi::default();
        let batch = Batch {
            taken: vec![delivery(Kind::Interrupt, "멈춰라")],
            inbox: Inbox::Steer,
            interrupt: true,
        };
        deliver(&api, &batch, Duration::ZERO).await.unwrap();
        let calls = api.calls.lock().unwrap();
        assert_eq!(calls[0], "interrupt");
        assert!(calls[1].starts_with("steer msg_"), "{}", calls[1]);
        assert!(calls[1].ends_with(
            "loom message #1 (from: 부모, kind: note, mode: interrupt. Stop your current work and do what this message says)\n멈춰라"
        ));
        assert_eq!(calls.len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn failed_prompt_retries_with_the_same_id() {
        let api = FakeApi {
            fail: 2,
            ..FakeApi::default()
        };
        let batch = Batch {
            taken: vec![delivery(Kind::Queue, "a")],
            inbox: Inbox::Queue,
            interrupt: false,
        };
        deliver(&api, &batch, PROMPT_RETRY_DELAY).await.unwrap();
        let calls = api.calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert!(calls.iter().all(|c| *c == calls[0]), "{calls:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn prompt_gives_up_after_bounded_attempts() {
        let api = FakeApi {
            fail: u32::MAX,
            ..FakeApi::default()
        };
        let batch = Batch {
            taken: vec![delivery(Kind::Queue, "a")],
            inbox: Inbox::Queue,
            interrupt: false,
        };
        let err = deliver(&api, &batch, PROMPT_RETRY_DELAY).await.unwrap_err();
        assert_eq!(err, "연결이 끊겼다");
        assert_eq!(api.calls.lock().unwrap().len(), PROMPT_ATTEMPTS as usize);
    }

    #[test]
    fn message_ids_match_the_server_pattern_and_sort_by_time() {
        let a = message_id();
        let b = message_id();
        assert!(a.starts_with("msg_") && a.len() == 4 + 12 + 14, "{a}");
        assert!(a[4..16] < b[4..16], "{a} {b}");
    }

    #[test]
    fn model_selection_of_this_session_gives_provider_model_and_variant() {
        let selected = json!({ "type": "session.model.selected", "data": {
            "sessionID": "ses_a", "model": { "id": "kta-chat-deep", "providerID": "KTA", "variant": "high" } } });
        assert_eq!(
            selected_model(&selected, "ses_a"),
            Some(Model {
                model: Some("KTA/kta-chat-deep".into()),
                effort: Some("high".into())
            })
        );
        let default = json!({ "type": "session.model.selected", "data": {
            "sessionID": "ses_a", "model": { "id": "kta-chat-deep", "providerID": "KTA" },
            "previous": { "id": "kta-chat-deep", "providerID": "KTA", "variant": "high" } } });
        assert_eq!(selected_model(&default, "ses_a").unwrap().effort, None);
        assert_eq!(selected_model(&selected, "ses_b"), None);
        let step = json!({ "type": "session.step.started", "data": { "sessionID": "ses_a",
            "model": { "id": "kta-chat-deep", "providerID": "KTA" } } });
        assert_eq!(selected_model(&step, "ses_a"), None);
    }

    fn asked_shell() -> Value {
        json!({ "type": "permission.asked", "location": { "directory": "/tmp/permprobe" }, "data": {
            "id": "per_1", "sessionID": "ses_a", "action": "shell", "resources": ["echo hi", "ls"],
            "save": ["echo *"], "source": { "type": "tool", "messageID": "msg_1", "id": "call_1" } } })
    }

    #[test]
    fn permission_events_of_this_session_give_the_request_and_its_reply() {
        let shell = Permission {
            id: "per_1".into(),
            action: "shell".into(),
            resources: vec!["echo hi".into(), "ls".into()],
        };
        assert_eq!(
            permission_event(&asked_shell(), "ses_a"),
            Some(PermissionEvent::Asked(shell.clone()))
        );
        assert_eq!(permission_event(&asked_shell(), "ses_b"), None);
        let replied = json!({ "type": "permission.replied", "data": {
            "sessionID": "ses_a", "requestID": "per_1", "reply": "reject" } });
        assert_eq!(
            permission_event(&replied, "ses_a"),
            Some(PermissionEvent::Replied {
                request: "per_1".into()
            })
        );
        assert_eq!(shell.summary(), "shell: echo hi");
        let prompt = shell.prompt();
        assert_eq!(prompt.summary, "shell: echo hi");
        assert_eq!(prompt.body, ["shell", "echo hi", "ls"]);
        let labels: Vec<&str> = prompt.options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(labels, ["Allow once", "Always allow", "Reject"]);
    }

    #[test]
    fn a_choice_decides_only_the_request_that_is_still_open() {
        let Some(PermissionEvent::Asked(asked)) = permission_event(&asked_shell(), "ses_a") else {
            panic!("not asked");
        };
        let shown = asked.prompt();
        let decided = |option| decide(Some(&asked), &shown, option);
        assert_eq!(
            [decided(0), decided(1), decided(2), decided(3)],
            [
                Some(PermissionReply::Once),
                Some(PermissionReply::Always),
                Some(PermissionReply::Reject),
                None,
            ]
            .map(|reply| reply.map(|reply| Decision {
                request: "per_1".into(),
                reply
            }))
        );
        assert_eq!(decide(None, &shown, 0), None);
        let next = Permission {
            id: "per_2".into(),
            ..asked.clone()
        };
        assert_eq!(decide(Some(&next), &shown, 0), None);
        assert_eq!(
            serde_json::to_value(PermissionReply::Always).unwrap(),
            json!("always")
        );
    }

    #[test]
    fn version_warning_ignores_the_leading_v() {
        assert_eq!(version_warning("opencode v2.0.32\n"), None);
        assert_eq!(
            version_warning("opencode v2.0.33\n"),
            Some(VersionMismatch {
                installed: "2.0.33".into(),
                pinned: "2.0.32".into()
            })
        );
    }

    #[test]
    fn args_attach_the_tui_to_the_node_session() {
        assert_eq!(
            args("http://127.0.0.1:1", "ses_1"),
            ["--server", "http://127.0.0.1:1", "--session", "ses_1"]
        );
    }
}
