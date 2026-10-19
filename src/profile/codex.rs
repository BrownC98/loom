//! Codex 프로필: 시작 인자(사용법 지시, app-server 주소)와 메일함 메시지를 `turn/*` 요청으로 바꾸는 규칙.
//! 바뀌는 이유는 Codex 버전이다. 요청·알림 타입은 `build.rs`가 고정한 스키마(`codex/schema/`)에서 만든다.
//! 스키마를 바꾸려면 `codex app-server generate-json-schema --out <폴더>`의 `v2/`에서 같은 이름의 파일을 옮기고
//! `PINNED_VERSION`을 고친다. 근거: docs/design.md 「에이전트 실행」, 「전달 지점」.

use std::future::Future;
use std::net::IpAddr;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::graph::{Delivery, Kind};

#[allow(clippy::all, dead_code)]
pub mod protocol {
    include!(concat!(env!("OUT_DIR"), "/codex_protocol.rs"));
}

use protocol::{
    TextUserInputType, ThreadTurnsListParams, ThreadTurnsListResponse, TurnInterruptParams,
    TurnItemsView, TurnStartParams, TurnStatus, TurnSteerParams, UserInput,
};

/// `src/profile/codex/schema/`를 만든 Codex 버전.
pub const PINNED_VERSION: &str = "0.159.2";
/// 새 턴 입력 하나에 넣는 본문 합의 상한. 첫 메시지는 늘 넣는다. 남은 메시지는 다음 턴에 들어간다.
/// Claude 훅 상한(`claude::HOOK_LIMIT_BYTES`)과 같게 둬 두 프로필의 메시지 묶음 크기를 맞춘다.
pub const INPUT_LIMIT_BYTES: usize = super::claude::HOOK_LIMIT_BYTES;

const USAGE: &str = "\
너는 loom 그래프의 노드다. loom은 부모·자식 노드 사이 메시지를 나른다. 명령은 셸로 부른다.
- `loom spawn <이름> --agent codex` (지시문은 stdin, `--file <경로>`): 자식 노드를 만든다. `--agent claude`도 된다. 샌드박스 안에서는 데몬에 닿지 않는다. 처음부터 샌드박스 밖 실행 승인(`require_escalated`)을 요청해 부른다.
- `loom send <이름> <본문>`: 부모나 자식에게 보낸다. 받는 노드의 턴을 끊지 않는다.
- `loom steer <이름> <본문>`: 일하는 자식의 턴에 넣는다.
- `loom interrupt <이름> <본문>`: 자식이 하던 일을 멈추게 하고 이 본문을 따르게 한다.
- `loom done <요약>` / `loom done --file <경로>`: 결과를 부모에게 보내고 끝낸다. 요약은 1KB, 본문은 4KB 이하다.
받은 메시지는 「loom 메시지」로 시작하는 입력으로 들어온다. 중단 메시지를 받으면 하던 일을 멈추고 그 본문을 따른다.
자식의 결과를 기다리는 동안 할 일이 없으면 턴을 끝낸다. 자식의 결과는 새 턴으로 들어온다. `loom wait`는 부르지 않는다.";

/// `codex` 시작 인자. 사용자의 `~/.codex` 파일을 고치지 않으려고 사용법은 `-c`로만 넘긴다.
/// `-c` 값은 TOML로 읽힌다. JSON 문자열은 TOML 기본 문자열과 같은 이스케이프를 쓴다.
pub fn args(app_server: &str) -> Vec<String> {
    let usage = serde_json::to_string(USAGE).expect("문자열은 늘 직렬화된다");
    vec![
        "--remote".into(),
        app_server.into(),
        "-c".into(),
        format!("developer_instructions={usage}"),
    ]
}

/// `codex app-server`의 `workspace-write` 샌드박스에 `daemon` 주소로 가는 길 하나만 연다.
/// 네트워크를 켜면 셸 명령은 Codex 프록시로만 밖에 닿고, 프록시는 `daemon` 말고 모든 주소를 막는다.
/// 권한 프로필(`default_permissions`)은 쓰지 않는다. TUI가 `thread/start`에 `sandbox`를 넣어 덮는다.
/// `version_output`이 고정 버전이 아니면 아무것도 넘기지 않는다. 프록시가 뜨지 않는 버전에서
/// `network_access`만 켜지면 모든 주소가 열린다. 근거: docs/design.md 「에이전트 실행」, 「실측 근거」.
pub fn sandbox_args(version_output: &str, daemon: IpAddr) -> Vec<String> {
    if version_output.split_whitespace().last() != Some(PINNED_VERSION) {
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
    let out = std::process::Command::new("codex")
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `codex --version` 출력(예: `codex-cli 0.159.2`)이 고정 버전과 다르면 화면에 보일 경고다.
pub fn version_warning(version_output: &str) -> Option<String> {
    let installed = version_output.split_whitespace().last()?;
    (installed != PINNED_VERSION).then(|| {
        format!("app-server {installed} · 고정 {PINNED_VERSION} · loom 명령마다 승인을 묻는다")
    })
}

/// 노드 스레드의 상태. `thread/status/changed` 알림이 정한다.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Idle,
    Busy,
}

/// 이 상태에서 메일함에서 꺼내는 전달 방식. 일하는 중에는 큐를 남긴다. 턴이 끝나면 `turn/start`로 넘긴다.
pub fn kinds(status: Status) -> &'static [Kind] {
    match status {
        Status::Idle => Kind::ALL,
        Status::Busy => &[Kind::Steer, Kind::Interrupt],
    }
}

/// 꺼낸 메시지를 `turn/*` 요청으로 넘긴다. `call`은 JSON-RPC 요청 하나를 보내고 결과를 돌려준다.
/// 꺼낸 메시지는 이미 `delivered`다. 그래서 첫 요청이 실패하면 다른 요청으로 한 번 더 넘긴다.
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
            from: "부모".into(),
            kind,
            body: body.into(),
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
                "loom 메시지 (보낸 노드: 부모, 방식: 큐)\na\n\nloom 메시지 (보낸 노드: 부모, 방식: 조정)\nb" }] })
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
            "loom 메시지 (보낸 노드: 부모, 방식: 중단. 하던 일을 멈추고 이 본문을 따른다)\n멈춰라"
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

    #[test]
    fn args_pass_usage_as_a_toml_string_and_version_warning_names_both_versions() {
        let args = args("ws://127.0.0.1:1");
        assert_eq!(args[..3], ["--remote", "ws://127.0.0.1:1", "-c"]);
        let (key, value) = args[3].split_once('=').unwrap();
        assert_eq!(key, "developer_instructions");
        assert_eq!(serde_json::from_str::<String>(value).unwrap(), USAGE);
        assert_eq!(version_warning("codex-cli 0.159.2"), None);
        assert_eq!(
            version_warning("codex-cli 0.160.0\n").as_deref(),
            Some("app-server 0.160.0 · 고정 0.159.2 · loom 명령마다 승인을 묻는다")
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
