use std::future::Future;
use std::net::IpAddr;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::VersionMismatch;
use crate::graph::{Delivery, Kind, Model, WaitKind};
use crate::screen::{Dialog, Grammar, Options, Place, Top};
use crate::skills::Launch;

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
    },
    Dialog {
        text: "Sign in with ChatGPT",
        kind: WaitKind::Auth,
        place: Place::BeforeSession,
    },
];
pub const INPUT_LIMIT_BYTES: usize = super::BATCH_LIMIT_BYTES;

pub(super) const WAITING: &str = "When waiting for children and no independent work remains, end your turn. Their messages start a new turn automatically. Do not run an unbounded loom wait.";

fn usage(launch: Launch) -> String {
    format!(
        "{}\n{WAITING}\n\n{}",
        super::COMMUNICATION_USAGE,
        crate::skills::always_on(launch)
    )
}

pub fn choice_args(choice: &super::ModelChoice) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(model) = &choice.model {
        args.extend(["-m".into(), model.clone()]);
    }
    if let Some(effort) = &choice.effort {
        let effort = serde_json::to_string(effort).expect("문자열은 늘 직렬화된다");
        args.extend(["-c".into(), format!("model_reasoning_effort={effort}")]);
    }
    args
}

pub fn args(app_server: &str) -> Vec<String> {
    vec!["--remote".into(), app_server.into()]
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
    super::installed_version("codex")
}

pub fn version_warning(version_output: &str) -> Option<VersionMismatch> {
    let installed = version_output.split_whitespace().last()?;
    (installed != PINNED_VERSION).then(|| VersionMismatch {
        installed: installed.to_owned(),
        pinned: PINNED_VERSION.to_owned(),
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Idle,
    Busy,
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

pub fn kinds(status: Status) -> &'static [Kind] {
    match status {
        Status::Idle => Kind::ALL,
        Status::Busy => &[Kind::Steer, Kind::Interrupt],
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::PromptKey;

    const APPROVAL: &str = concat!(
        "\n",
        "  >_ OpenAI Codex (v0.159.2)\n",
        "     /tmp/zz/repo\n",
        "\n",
        "  What’s the first thing on our napkin sketch?\n",
        "\n",
        "\n",
        "› Run exactly this shell command: touch zz_file\n",
        "\n",
        "\n",
        "• 요청한 명령을 실행하겠습니다.\n",
        "\n",
        "• Running touch zz_file\n",
        "\n",
        "\n",
        "  Would you like to run the following command?\n",
        "\n",
        "  Environment: local\n",
        "\n",
        "  Reason: 읽기 전용 제한으로 명령이 실패했습니다. 제한 밖에서 같은 명령을 실행하도록 허용하시겠습니까?\n",
        "\n",
        "  $ touch zz_file\n",
        "\n",
        "\n",
        "› 1. Yes, proceed (y)\n",
        "  2. Yes, and don't ask again for commands that start with `touch zz_file` (p)\n",
        "  3. No, and tell Codex what to do differently (esc)\n",
        "\n",
        "  Press enter to confirm or esc to cancel\n",
    );

    const NARROW: &str = concat!(
        "  Add --image <path> for each image that the human gave. The human sees results\n",
        "  only through loom done. Do not call it when the input is only a question and\n",
        "  you only answer in text.\n",
        "\n",
        "\n",
        "• Running curl -sI https://example.com\n",
        "\n",
        "\n",
        "  Would you like to run the following command?\n",
        "\n",
        "  Environment: local\n",
        "\n",
        "  Reason: May I run the requested curl command with network access outside the\n",
        "  sandbox?\n",
        "\n",
        "  $ curl -sI https://example.com\n",
        "\n",
        "\n",
        "› 1. Yes, proceed (y)\n",
        "  2. Yes, and don't ask again for commands that start with `curl -sI https://\n",
        "     example.com` (p)\n",
        "  3. No, and tell Codex what to do differently (esc)\n",
        "\n",
        "  Press enter to confirm or esc to cancel\n",
    );

    #[test]
    fn wrapped_reason_and_option_are_joined() {
        let prompt = crate::screen::prompt(&PROMPT, NARROW, 80).unwrap();
        assert_eq!(
            prompt.body[2],
            "Reason: May I run the requested curl command with network access outside the sandbox?"
        );
        assert_eq!(
            prompt.options[1].label,
            "Yes, and don't ask again for commands that start with `curl -sI https://example.com`"
        );
    }

    #[test]
    fn approval_prompt_is_read_from_the_screen() {
        let prompt = crate::screen::prompt(&PROMPT, APPROVAL, 120).unwrap();
        assert_eq!(
            prompt.body[0],
            "Would you like to run the following command?"
        );
        assert_eq!(prompt.summary, "$ touch zz_file");
        let options: Vec<_> = prompt
            .options
            .iter()
            .map(|o| (o.label.as_str(), o.key))
            .collect();
        assert_eq!(
            options,
            [
                ("Yes, proceed", PromptKey::Char('1')),
                (
                    "Yes, and don't ask again for commands that start with `touch zz_file`",
                    PromptKey::Char('2')
                ),
                (
                    "No, and tell Codex what to do differently",
                    PromptKey::Char('3')
                ),
            ]
        );
    }

    #[test]
    fn launch_text_has_the_ask_rules() {
        super::super::assert_every_launch_asks(|launch| (usage(launch), usage(launch)));
    }
    use serde_json::json;
    use std::sync::Mutex;

    fn delivery(kind: Kind, body: &str) -> Delivery {
        Delivery {
            id: crate::graph::MessageId(1),
            from: "부모".into(),
            kind,
            body: body.into(),
            what: Some(crate::protocol::MessageKind::Note),
        }
    }

    struct Fake {
        in_progress: Option<&'static str>,
        fail: &'static [&'static str],
        sent: Mutex<Vec<(&'static str, Value)>>,
    }

    impl Fake {
        fn new(in_progress: Option<&'static str>, fail: &'static [&'static str]) -> Self {
            Self {
                in_progress,
                fail,
                sent: Mutex::default(),
            }
        }

        async fn call(&self, method: &'static str, params: Value) -> Result<Value, String> {
            if method == "thread/turns/list" {
                let data = match self.in_progress {
                    Some(id) => {
                        json!([{ "id": id, "items": [], "itemsView": "notLoaded", "status": "inProgress", "error": null }])
                    }
                    None => {
                        json!([{ "id": "old", "items": [], "itemsView": "notLoaded", "status": "completed", "error": null }])
                    }
                };
                return Ok(json!({ "data": data }));
            }
            self.sent.lock().unwrap().push((method, params));
            if self.fail.contains(&method) {
                return Err(format!("{method} 실패"));
            }
            Ok(json!({}))
        }

        fn methods(&self) -> Vec<&'static str> {
            self.sent.lock().unwrap().iter().map(|(m, _)| *m).collect()
        }
    }

    async fn run(fake: &Fake, status: Status, taken: &[Delivery]) -> Result<(), String> {
        deliver(|m, p| fake.call(m, p), "th", status, taken).await
    }

    #[test]
    fn busy_nodes_keep_the_queue_for_the_next_turn() {
        assert_eq!(kinds(Status::Idle), Kind::ALL);
        assert_eq!(kinds(Status::Busy), [Kind::Steer, Kind::Interrupt]);
    }

    #[tokio::test]
    async fn idle_node_gets_every_message_in_one_turn_start() {
        let fake = Fake::new(None, &[]);
        let taken = [delivery(Kind::Queue, "a"), delivery(Kind::Steer, "b")];
        run(&fake, Status::Idle, &taken).await.unwrap();
        let sent = fake.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "turn/start");
        assert_eq!(
            sent[0].1,
            json!({ "threadId": "th", "input": [{ "type": "text", "text":
                "loom message #1 (from: 부모, kind: note)\na\n\nloom message #1 (from: 부모, kind: note, mode: steer)\nb" }] })
        );
    }

    #[tokio::test]
    async fn steer_goes_into_the_running_turn() {
        let fake = Fake::new(Some("t1"), &[]);
        run(&fake, Status::Busy, &[delivery(Kind::Steer, "b")])
            .await
            .unwrap();
        let sent = fake.sent.lock().unwrap();
        assert_eq!(sent[0].0, "turn/steer");
        assert_eq!(sent[0].1["expectedTurnId"], "t1");
        assert_eq!(sent.len(), 1);
    }

    #[tokio::test]
    async fn steer_that_misses_the_turn_starts_a_new_one() {
        let fake = Fake::new(None, &[]);
        run(&fake, Status::Busy, &[delivery(Kind::Steer, "b")])
            .await
            .unwrap();
        assert_eq!(fake.methods(), ["turn/start"]);
        let fake = Fake::new(Some("t1"), &["turn/steer"]);
        run(&fake, Status::Busy, &[delivery(Kind::Steer, "b")])
            .await
            .unwrap();
        assert_eq!(fake.methods(), ["turn/steer", "turn/start"]);
    }

    #[tokio::test]
    async fn interrupt_stops_the_turn_then_starts_one_with_the_body() {
        let fake = Fake::new(Some("t1"), &[]);
        let taken = [delivery(Kind::Interrupt, "멈춰라")];
        run(&fake, Status::Busy, &taken).await.unwrap();
        let sent = fake.sent.lock().unwrap();
        assert_eq!(
            sent[0],
            (
                "turn/interrupt",
                json!({ "threadId": "th", "turnId": "t1" })
            )
        );
        assert_eq!(sent[1].0, "turn/start");
        assert_eq!(
            sent[1].1["input"][0]["text"],
            "loom message #1 (from: 부모, kind: note, mode: interrupt. Stop your current work and do what this message says)\n멈춰라"
        );
    }

    #[tokio::test]
    async fn start_that_races_a_new_turn_steers_into_it() {
        let fake = Fake::new(Some("t2"), &["turn/start"]);
        run(&fake, Status::Idle, &[delivery(Kind::Queue, "a")])
            .await
            .unwrap();
        assert_eq!(fake.methods(), ["turn/start", "turn/steer"]);
        let fake = Fake::new(None, &["turn/start"]);
        let err = run(&fake, Status::Idle, &[delivery(Kind::Queue, "a")])
            .await
            .unwrap_err();
        assert!(err.starts_with("turn/start 실패"), "{err}");
    }

    #[tokio::test]
    async fn last_reply_is_the_last_agent_message_of_the_last_turn() {
        let sent = Mutex::new(Vec::new());
        let call = |method: &'static str, params: Value| {
            sent.lock().unwrap().push((method, params));
            async move {
                Ok(
                    json!({ "data": [{ "id": "t", "itemsView": "full", "status": "completed", "error": null, "items": [
                    { "type": "userMessage", "id": "1", "content": [] },
                    { "type": "agentMessage", "id": "2", "text": "살펴본다." },
                    { "type": "agentMessage", "id": "3", "text": "병합할까요?" },
                ] }] }),
                )
            }
        };
        assert_eq!(
            last_reply(call, "th").await.unwrap().as_deref(),
            Some("병합할까요?")
        );
        let sent = sent.into_inner().unwrap();
        assert_eq!(sent[0].0, "thread/turns/list");
        assert_eq!(sent[0].1["limit"], 1);
        assert_eq!(sent[0].1["itemsView"], "full");
    }

    #[tokio::test]
    async fn model_comes_from_thread_read() {
        let thread = json!({ "thread": {
            "id": "th", "cliVersion": "0.159.2", "createdAt": 1, "updatedAt": 2, "cwd": "/w",
            "ephemeral": false, "modelProvider": "openai", "preview": "", "projectId": "p",
            "sessionId": "s", "source": "cli", "status": { "type": "idle" }, "turns": [],
            "model": "gpt-6-luna", "reasoningEffort": "high",
        } });
        let sent = Mutex::new(Vec::new());
        let call = |method: &'static str, params: Value| {
            sent.lock().unwrap().push((method, params));
            let reply = thread.clone();
            async move { Ok(reply) }
        };
        assert_eq!(
            model(call, "th").await.unwrap(),
            Model {
                model: Some("gpt-6-luna".into()),
                effort: Some("high".into())
            }
        );
        assert_eq!(
            sent.lock().unwrap()[0],
            (
                "thread/read",
                json!({ "threadId": "th", "includeTurns": false })
            )
        );
    }

    #[test]
    fn server_args_pass_usage_as_a_toml_string() {
        let args = server_args(crate::skills::Launch {
            position: crate::skills::Position::Child,
            skills: Default::default(),
        });
        assert_eq!(args[0], "-c");
        let (key, value) = args[1].split_once('=').unwrap();
        assert_eq!(key, "developer_instructions");
        assert_eq!(
            serde_json::from_str::<String>(value).unwrap(),
            usage(crate::skills::Launch {
                position: crate::skills::Position::Child,
                skills: Default::default()
            })
        );
        assert_eq!(
            super::args("ws://127.0.0.1:1"),
            ["--remote", "ws://127.0.0.1:1"]
        );
        assert_eq!(version_warning("codex-cli 0.159.2"), None);
        assert_eq!(
            version_warning("codex-cli 0.160.0\n"),
            Some(VersionMismatch {
                installed: "0.160.0".into(),
                pinned: "0.159.2".into()
            })
        );
    }

    #[test]
    fn sandbox_opens_the_daemon_address_only_on_the_pinned_version() {
        let daemon = IpAddr::from([127, 76, 77, 1]);
        let pinned: &[&str] = if cfg!(windows) {
            &[]
        } else {
            &[
                "-c",
                "sandbox_workspace_write.network_access=true",
                "-c",
                "features.network_proxy.enabled=true",
                "-c",
                r#"features.network_proxy.domains={"127.76.77.1"="allow"}"#,
            ]
        };
        assert_eq!(sandbox_args("codex-cli 0.159.2\n", daemon), pinned);
        assert!(sandbox_args("codex-cli 0.160.0\n", daemon).is_empty());
        assert!(sandbox_args("", daemon).is_empty());
    }
}
