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

/// 승인 창(0.159.2 실측): 테두리가 없고 「Would you like to …」 줄부터 본문이다. 선택지는 `› 1. Yes, proceed (y)` 꼴이다.
pub const PROMPT: Grammar = Grammar {
    top: Top::Line("Would you like to"),
    options: Options::Numbered,
    guard: None,
};

/// `src/profile/codex/schema/`를 만든 Codex 버전.
pub const PINNED_VERSION: &str = "0.159.2";
/// app-server가 알리지 않는 TUI 확인 창(0.159.2 화면 문구). 승인·답 대기는 스레드 상태가 알린다(`codex_server::Thread`).
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
/// 새 턴 입력 하나에 넣는 본문 합의 상한. 첫 메시지는 늘 넣는다. 남은 메시지는 다음 턴에 들어간다.
pub const INPUT_LIMIT_BYTES: usize = super::BATCH_LIMIT_BYTES;

/// 사용법 끝에 붙는 대기 방식. 바꾸면 뜬 노드가 `rules-changed`를 받는다(`profile::usage_digest`).
pub(super) const WAITING: &str = "When waiting for children and no independent work remains, end your turn. Their messages start a new turn automatically. Do not run an unbounded loom wait.";

fn usage(launch: Launch) -> String {
    format!(
        "{}\n{WAITING}\n\n{}",
        super::COMMUNICATION_USAGE,
        crate::skills::always_on(launch)
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

pub fn args(app_server: &str) -> Vec<String> {
    vec!["--remote".into(), app_server.into()]
}

/// `codex app-server` 시작 인자. 사용자의 `~/.codex` 파일을 고치지 않으려고 사용법은 `-c`로만 넘긴다.
/// TUI(`--remote`)에 넘긴 `developer_instructions`는 서버가 만든 스레드에 들어가지 않는다(0.159.2 실측).
/// `-c` 값은 TOML로 읽힌다. JSON 문자열은 TOML 기본 문자열과 같은 이스케이프를 쓴다.
pub fn server_args(launch: Launch) -> Vec<String> {
    let usage = serde_json::to_string(&usage(launch)).expect("문자열은 늘 직렬화된다");
    vec!["-c".into(), format!("developer_instructions={usage}")]
}

/// 계정 전환으로 새 서버에서 이어 갈 스레드를 TUI보다 먼저 연다(`thread/resume`). 권한은 처음 띄울 때 TUI가 쓴 값과 같게 둔다.
/// `codex resume --remote`는 권한 인자를 받지 않고, 신뢰하지 않은 폴더에서는 `read-only`로 이었다(0.159.2 실측).
/// `bypass`는 모든 권한 허용 인자로 띄운 노드다. 아니면 Codex TUI 기본(`workspace-write`, `on-request`)이다.
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

/// 마지막 턴의 마지막 답 글. 턴 끝 질문 검사와 심플 카드 상태 줄이 쓴다. 이 연결은 구독하지 않아 답 알림(`item/completed`)을 받지 못한다.
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

/// 쉬는 스레드에 새 턴을 연다. 턴 끝 질문 검사가 다시 일하게 할 때 쓴다.
pub async fn start_turn<F, Fut>(call: F, thread: &str, text: &str) -> Result<(), String>
where
    F: Fn(&'static str, Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    Rpc { call, thread }.start(input(text.to_owned())).await
}

/// 진행 중인 턴에 글 하나를 더한다. 사람 입력 턴에 열린 질문을 알릴 때 쓴다.
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
    let input = input(super::message_text(taken));
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

    /// 마지막 턴 하나를 읽는 요청. 서버는 새 턴부터 준다.
    fn last_turn(&self, items: TurnItemsView) -> ThreadTurnsListParams {
        ThreadTurnsListParams {
            thread_id: self.thread.to_owned(),
            limit: Some(1),
            cursor: None,
            items_view: Some(items),
            sort_direction: None,
        }
    }

    /// 진행 중인 턴의 id. 턴은 TUI도 시작하므로 그때마다 서버에서 읽는다.
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

    /// 0.159.2 승인 창(2026-10-23 tmux 120x40 실측, `-a on-request -s read-only`).
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

    /// 노드 터미널(80x24)의 승인 창(2026-10-23 시험 데몬 실측). 「Reason:」 줄과 주소가 든 선택지가 넘어간다.
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

    /// 마지막 턴의 마지막 `agentMessage`가 답 글이다. 요청은 마지막 턴 하나를 항목과 함께 읽는다.
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
