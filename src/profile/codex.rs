//! Codex 프로필: 시작 인자(사용법 지시, app-server 주소)와 메일함 메시지를 `turn/*` 요청으로 바꾸는 규칙.
//! 바뀌는 이유는 Codex 버전이다. 요청·알림 타입은 `build.rs`가 고정한 스키마(`codex/schema/`)에서 만든다.
//! 스키마를 바꾸려면 `codex app-server generate-json-schema --out <폴더>`의 `v2/`에서 같은 이름의 파일을 옮기고
//! `PINNED_VERSION`을 고친다. 근거: docs/design.md 「에이전트 실행」, 「전달 지점」.

use std::future::Future;
use std::net::IpAddr;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::VersionMismatch;
use crate::graph::{Delivery, Kind, Model};

#[allow(clippy::all, dead_code)]
pub mod protocol {
    include!(concat!(env!("OUT_DIR"), "/codex_protocol.rs"));
}

use protocol::{
    AbsolutePathBuf, SkillsExtraRootsSetParams, TextUserInputType, Thread, ThreadReadParams,
    ThreadReadResponse, ThreadTurnsListParams, ThreadTurnsListResponse, TurnInterruptParams,
    TurnItemsView, TurnStartParams, TurnStatus, TurnSteerParams, UserInput,
};

/// `src/profile/codex/schema/`를 만든 Codex 버전.
pub const PINNED_VERSION: &str = "0.159.2";
/// 새 턴 입력 하나에 넣는 본문 합의 상한. 첫 메시지는 늘 넣는다. 남은 메시지는 다음 턴에 들어간다.
pub const INPUT_LIMIT_BYTES: usize = super::BATCH_LIMIT_BYTES;

fn usage() -> String {
    format!(
        "{}\nWhen waiting for children and no independent work remains, end your turn. Their messages start a new turn automatically. Do not run an unbounded loom wait.\n\n{}",
        super::COMMUNICATION_USAGE,
        crate::skills::always_on()
    )
}

/// `-m`과 `model_reasoning_effort` 시작 인자. `-c` 값은 TOML이라 문자열을 따옴표로 감싼다.
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

/// `codex` 시작 인자. 터미널 제목은 스레드 제목만 쓴다. 기본 제목(`<스피너> | <폴더>`)에는 카드 주제가 없다(`Profile::topic`).
pub fn args(app_server: &str) -> Vec<String> {
    vec![
        "--remote".into(),
        app_server.into(),
        "-c".into(),
        r#"tui.terminal_title=["thread-title"]"#.into(),
    ]
}

/// `codex app-server` 시작 인자. 사용자의 `~/.codex` 파일을 고치지 않으려고 사용법은 `-c`로만 넘긴다.
/// TUI(`--remote`)에 넘긴 `developer_instructions`는 서버가 만든 스레드에 들어가지 않는다(0.159.2 실측).
/// `-c` 값은 TOML로 읽힌다. JSON 문자열은 TOML 기본 문자열과 같은 이스케이프를 쓴다.
pub fn server_args() -> Vec<String> {
    let usage = serde_json::to_string(&usage()).expect("문자열은 늘 직렬화된다");
    vec!["-c".into(), format!("developer_instructions={usage}")]
}

/// `codex app-server`의 `workspace-write` 샌드박스에 `daemon` 주소로 가는 길 하나만 연다.
/// 네트워크를 켜면 셸 명령은 Codex 프록시로만 밖에 닿고, 프록시는 `daemon` 말고 모든 주소를 막는다.
/// 권한 프로필(`default_permissions`)은 쓰지 않는다. TUI가 `thread/start`에 `sandbox`를 넣어 덮는다.
/// `version_output`이 고정 버전이 아니면 아무것도 넘기지 않는다. 프록시가 뜨지 않는 버전에서
/// `network_access`만 켜지면 모든 주소가 열린다. 근거: docs/design.md 「에이전트 실행」, 「실측 근거」.
/// Windows도 아무것도 넘기지 않는다. 프록시는 관리자 권한 샌드박스(elevated)에서만 뜨고, 기본 Windows Codex는
/// `workspace-write`를 `read-only`로 낮춘다(실측). 그때 Codex 노드는 loom 명령마다 승인을 묻는다.
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

/// 데몬 PATH의 `codex --version` 출력. 설치하지 않았으면 없다.
pub fn installed_version() -> Option<String> {
    super::installed_version("codex")
}

/// `codex --version` 출력(예: `codex-cli 0.159.2`)이 고정 버전과 다르면 화면에 보일 경고다.
/// 다르면 메시지 연동이 맞지 않을 수 있고 loom 명령마다 승인을 묻는다.
pub fn version_warning(version_output: &str) -> Option<VersionMismatch> {
    let installed = version_output.split_whitespace().last()?;
    (installed != PINNED_VERSION).then(|| VersionMismatch {
        installed: installed.to_owned(),
        pinned: PINNED_VERSION.to_owned(),
    })
}

/// 노드 스레드의 상태. `thread/status/changed` 알림이 정한다.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Idle,
    Busy,
}

/// 노드 스레드의 지금 모델과 추론 강도. `call`은 JSON-RPC 요청 하나를 보내고 결과를 돌려준다.
/// TUI가 `/model`로 바꾼 값은 다음 턴에 스레드 설정이 된다. 이 연결은 구독하지 않아 설정 알림
/// (`thread/settings/updated`)을 받지 못한다(실측). 그래서 스레드 상태가 바뀔 때마다 데몬이 부른다.
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

/// loom이 싣는 스킬 폴더(`skills::Plugin::skills`)를 서버의 스킬 폴더에 더한다. Codex에는 시작 인자로 스킬 폴더를
/// 더하는 방법이 없다. 서버가 가진 값이라 TUI가 만드는 노드 스레드도 이 스킬을 본다(0.159.2 실측).
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

/// 이 상태에서 메일함에서 꺼내는 전달 방식. 일하는 중에는 큐를 남긴다. 턴이 끝나면 `turn/start`로 넘긴다.
pub fn kinds(status: Status) -> &'static [Kind] {
    match status {
        Status::Idle => Kind::ALL,
        Status::Busy => &[Kind::Steer, Kind::Interrupt],
    }
}

/// 꺼낸 메시지를 `turn/*` 요청으로 넘긴다. `call`은 JSON-RPC 요청 하나를 보내고 결과를 돌려준다.
/// 첫 요청이 실패하면 다른 요청으로 한 번 더 넘긴다. 그래도 실패하면 받음을 확인하지 않고, 데몬이 `REDELIVER_AFTER` 뒤에 다시 꺼낸다.
/// - 쉬는 중: `turn/start`. 그 사이 턴이 시작돼 실패하면 그 턴에 `turn/steer`.
/// - 일하는 중, 조정: `turn/steer`. 그 사이 턴이 끝나 실패하면 `turn/start`.
/// - 일하는 중, 중단: `turn/interrupt` 뒤 `turn/start`.
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
    let input = vec![UserInput::TextUserInput {
        text: super::message_text(taken),
        text_elements: Vec::new(),
        type_: TextUserInputType::Text,
    }];
    match status {
        Status::Idle => rpc.start_or_steer(input).await,
        Status::Busy if taken.iter().any(|d| d.kind == Kind::Interrupt) => {
            if let Some(turn) = rpc.current_turn().await? {
                let params = TurnInterruptParams {
                    thread_id: thread.to_owned(),
                    turn_id: turn,
                };
                // 그 사이 턴이 끝났으면 실패한다. 새 턴은 그래도 시작한다.
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

    /// 진행 중인 턴의 id. 턴은 TUI도 시작하므로 그때마다 서버에서 읽는다.
    async fn current_turn(&self) -> Result<Option<String>, String> {
        let params = ThreadTurnsListParams {
            thread_id: self.thread.to_owned(),
            limit: Some(1),
            cursor: None,
            items_view: Some(TurnItemsView::NotLoaded),
            sort_direction: None,
        };
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
    use serde_json::json;
    use std::sync::Mutex;

    fn delivery(kind: Kind, body: &str) -> Delivery {
        Delivery {
            id: crate::graph::MessageId(1),
            from: "부모".into(),
            kind,
            body: body.into(),
            reply_to: None,
        }
    }

    /// 보낸 요청을 적는 가짜 서버. `in_progress`가 있으면 그 턴이 진행 중이다. `fail`에 있는 메서드는 실패한다.
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
                "loom message #1 (from: 부모, mode: queue)\na\n\nloom message #1 (from: 부모, mode: steer)\nb" }] })
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
        // 꺼낸 뒤 턴이 끝났다: 진행 중인 턴이 없다.
        let fake = Fake::new(None, &[]);
        run(&fake, Status::Busy, &[delivery(Kind::Steer, "b")])
            .await
            .unwrap();
        assert_eq!(fake.methods(), ["turn/start"]);
        // 턴 id는 읽었지만 그 사이 끝나 서버가 거부했다.
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
            "loom message #1 (from: 부모, mode: interrupt. Stop current work and follow this body)\n멈춰라"
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

    /// 0.159.2 `thread/read` 응답의 모양이다(필수 필드만 남겼다). TUI `/model` 뒤 다음 턴에 이 값이 바뀐다(실측).
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
        let args = server_args();
        assert_eq!(args[0], "-c");
        let (key, value) = args[1].split_once('=').unwrap();
        assert_eq!(key, "developer_instructions");
        assert_eq!(serde_json::from_str::<String>(value).unwrap(), usage());
        assert_eq!(
            super::args("ws://127.0.0.1:1"),
            [
                "--remote",
                "ws://127.0.0.1:1",
                "-c",
                r#"tui.terminal_title=["thread-title"]"#
            ]
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
        assert_eq!(
            sandbox_args("codex-cli 0.159.2\n", daemon),
            [
                "-c",
                "sandbox_workspace_write.network_access=true",
                "-c",
                "features.network_proxy.enabled=true",
                "-c",
                r#"features.network_proxy.domains={"127.76.77.1"="allow"}"#,
            ]
        );
        assert!(sandbox_args("codex-cli 0.160.0\n", daemon).is_empty());
        assert!(sandbox_args("", daemon).is_empty());
    }
}
