//! Cursor CLI 프로필: 시작 인자, 플러그인 훅 설정, 훅 입출력 형식.
//! 바뀌는 이유는 Cursor CLI 버전이다. 형식은 https://cursor.com/docs/agent/hooks 를 따른다.
//! 실측: cursor-agent 2026.10.19-6bbf2c4. 근거: docs/design.md 「에이전트 실행」, 「전달 지점」.

use std::path::Path;

use serde::Deserialize;
use serde_json::{Value, json};

use super::Take;
use crate::graph::{Delivery, Kind, Model, WaitKind};
use crate::screen::Dialog;
use crate::skills::Position;

/// 훅이 알리지 않는 TUI 확인 창(2026.10.19 화면 문구). 폴더 신뢰·명령 승인·로그인·질문은 실측, 나머지는 번들 글이다.
/// "Run this command"는 샌드박스 밖 실행 창("Run this command outside the sandbox?")도 맞춘다.
/// 질문 도구(AskQuestion)는 preToolUse 훅을 부르지 않는다(실측). 그래서 질문도 화면으로 찾는다.
pub const DIALOGS: [Dialog; 8] = [
    Dialog {
        // 창 제목(예: "Clarifying Questions")은 모델이 정한다. 도움말 줄은 고정이다(실측).
        text: "Enter next/submit",
        kind: WaitKind::Question,
        at_end: false,
    },
    Dialog {
        text: "Do you trust the contents of this directory?",
        kind: WaitKind::Trust,
        at_end: false,
    },
    Dialog {
        text: "Run this command",
        kind: WaitKind::Approval,
        at_end: false,
    },
    Dialog {
        text: "Run this MCP tool?",
        kind: WaitKind::Approval,
        at_end: false,
    },
    Dialog {
        text: "Read this file?",
        kind: WaitKind::Approval,
        at_end: false,
    },
    Dialog {
        text: "Write to this file?",
        kind: WaitKind::Approval,
        at_end: false,
    },
    Dialog {
        text: "Delete this file?",
        kind: WaitKind::Approval,
        at_end: false,
    },
    Dialog {
        text: "Press any key to log in",
        kind: WaitKind::Auth,
        at_end: false,
    },
];

/// 노드 프로세스에 넣는 시작 자리. 훅은 모든 노드가 같은 파일을 쓰므로 자리를 환경 변수로 받는다.
pub const POSITION_ENV: &str = "LOOM_POSITION";
/// Cursor는 훅 `additional_context` 한 개가 이 글자 수를 넘으면 버린다(실측 번들의 상한).
/// 그래서 사용법과 상시 지시를 sessionStart 훅 둘로 나눈다. 부모 없는 노드의 상시 지시는 약 11,000자다.
#[cfg(test)]
const CONTEXT_LIMIT_CHARS: usize = 10_000;
/// 훅 한 번에 넘기는 본문 합의 상한.
pub const HOOK_LIMIT_BYTES: usize = super::BATCH_LIMIT_BYTES;
/// 턴 안의 훅 상한. `loom` 명령의 데몬 응답 상한(`claude::HOOK_HTTP_TIMEOUT_SECS`)보다 길다.
const HOOK_TIMEOUT_SECS: u64 = 10;
/// 쉬는 노드의 stop 훅 상한. Claude 비동기 훅과 같은 근거다(JavaScript 타이머 상한 아래).
/// stop 훅이 기다리는 동안에도 사람은 입력해 새 턴을 띄울 수 있다(실측).
const IDLE_HOOK_TIMEOUT_SECS: u64 = 2_000_000;
/// 플러그인 표지가 가리키는 훅 파일. Claude는 `.cursor-plugin`과 이 파일을 읽지 않는다.
const HOOKS_FILE: &str = "cursor-hooks.json";
const MANIFEST: &str = r#"{"name":"loom","description":"loom이 싣는 스킬과 메시지 훅","skills":"./skills/","hooks":"./cursor-hooks.json"}"#;
/// Shell 명령 앞에 붙이는 줄. 셸을 중단 대상으로 데몬에 알린다(`claude::GUARD`와 같다).
const GUARD: &str = "loom guard";

/// loom 플러그인 폴더에 Cursor 표지와 훅 파일을 쓴다. Cursor는 `.cursor-plugin`을 `.claude-plugin`보다 먼저 읽는다.
pub fn write_plugin(dir: &Path) -> std::io::Result<()> {
    let manifest = dir.join(".cursor-plugin").join("plugin.json");
    std::fs::create_dir_all(manifest.parent().expect("파일 경로에는 부모가 있다"))?;
    std::fs::write(manifest, MANIFEST)?;
    std::fs::write(dir.join(HOOKS_FILE), hooks().to_string())
}

/// 훅 설정. 쉬는 노드를 깨우는 stop 훅은 `loop_limit`이 없어야 한다(기본값이면 몇 번 뒤 부르지 않는다).
fn hooks() -> Value {
    let hook = json!([{ "command": "loom cursor-hook", "timeout": HOOK_TIMEOUT_SECS }]);
    json!({
        "version": 1,
        "hooks": {
            "sessionStart": [
                { "command": "loom cursor-hook", "timeout": HOOK_TIMEOUT_SECS },
                { "command": "loom cursor-rules", "timeout": HOOK_TIMEOUT_SECS },
            ],
            "beforeSubmitPrompt": hook,
            "preToolUse": hook,
            "postToolUse": hook,
            "postToolUseFailure": hook,
            "stop": [{ "command": "loom cursor-hook", "timeout": IDLE_HOOK_TIMEOUT_SECS, "loop_limit": null }],
        },
    })
}

/// `cursor-agent` 시작 인자. 사용자의 `~/.cursor` 파일을 고치지 않으려고 훅과 스킬을 플러그인 폴더로만 넘긴다.
pub fn args(plugin: &Path) -> Vec<String> {
    vec!["--plugin-dir".into(), plugin.to_string_lossy().into_owned()]
}

/// 시작 자리를 환경 변수 값으로.
pub fn position_value(position: Position) -> &'static str {
    match position {
        Position::Root => "root",
        Position::Child => "child",
    }
}

/// `loom cursor-rules`가 노드 환경에서 읽는다. 값이 없으면 자식으로 본다(총괄 절차를 주지 않는다).
pub fn position_from(value: Option<&str>) -> Position {
    match value {
        Some("root") => Position::Root,
        _ => Position::Child,
    }
}

/// 첫 sessionStart 훅의 글: 통신 명령과 Cursor 대기 방식.
pub fn usage() -> String {
    format!(
        "{}\nWhen no independent work remains, end your turn. loom wakes you with a new message when one arrives. Cursor runs that wake hook only when ~/.cursor/hooks.json has a \"stop\" entry; if it has none, wait for an expected reply with loom wait --reply-to <message-id> --timeout <seconds> before you end your turn. The loom guard line before shell commands is inserted by loom; do not remove it or add it yourself.",
        super::COMMUNICATION_USAGE
    )
}

/// 둘째 sessionStart 훅의 출력: 상시 지시.
pub fn rules(position: Position) -> Value {
    json!({ "additional_context": crate::skills::always_on(position) })
}

/// `--model` 시작 인자. Cursor 모델 id는 추론 강도를 이름 끝에 붙인다(`--list-models`, 예: `claude-opus-5-5-high`).
/// 강도만 고르면 붙일 모델이 없다.
pub fn choice_args(choice: &super::ModelChoice) -> Option<Vec<String>> {
    let model = match (&choice.model, &choice.effort) {
        (Some(model), Some(effort)) => format!("{model}-{effort}"),
        (Some(model), None) => model.clone(),
        (None, _) => return None,
    };
    Some(vec!["--model".into(), model])
}

/// 훅 입력에서 loom이 보는 필드만 읽는다. 다른 필드는 버린다.
#[derive(Deserialize)]
#[serde(tag = "hook_event_name", rename_all = "camelCase")]
pub enum Event {
    SessionStart {
        model: Option<String>,
    },
    BeforeSubmitPrompt {
        model: Option<String>,
    },
    PreToolUse {
        tool_name: String,
        tool_input: Value,
    },
    PostToolUse,
    PostToolUseFailure,
    Stop {
        model: Option<String>,
    },
}

impl Event {
    /// 이 지점에서 꺼내는 방법. Claude와 같은 표다(`claude::Event::take`).
    /// sessionStart는 꺼내지 않는다. Cursor는 그 훅이 끝나야 첫 턴을 띄운다. 쉬는 동안의 메시지는 첫 stop 훅이 넘긴다.
    pub fn take(&self) -> Option<Take> {
        Some(match self {
            Event::SessionStart { .. } => return None,
            Event::Stop { .. } => Take::WhenIdle,
            Event::BeforeSubmitPrompt { .. } => Take::Now(Kind::ALL),
            Event::PostToolUse => Take::Now(&[Kind::Steer]),
            Event::PreToolUse { .. } | Event::PostToolUseFailure => Take::Now(&[Kind::Interrupt]),
        })
    }

    /// 훅 입력이 알리는 모델 id(강도 포함). 도구 훅의 `model`은 강도를 뺀 이름이라 쓰지 않는다(실측).
    pub fn model(&self) -> Option<Model> {
        match self {
            Event::SessionStart { model }
            | Event::BeforeSubmitPrompt { model }
            | Event::Stop { model } => Some(Model {
                model: model.clone(),
                effort: None,
            }),
            _ => None,
        }
    }

    /// 훅 출력 JSON. 쉬는 노드(stop)의 메시지는 새 턴 입력(`followup_message`)으로 넘긴다.
    pub fn reply(&self, taken: &[Delivery]) -> Value {
        if taken.is_empty() {
            return match self {
                Event::SessionStart { .. } => json!({ "additional_context": usage() }),
                Event::BeforeSubmitPrompt { .. } => json!({ "continue": true }),
                Event::PreToolUse {
                    tool_name,
                    tool_input,
                } => match guarded(tool_name, tool_input) {
                    // `permission`을 넣지 않는다. 넣으면 사람이 끈 모든 권한 허용을 훅이 대신 허용한다.
                    Some(input) => json!({ "updated_input": input }),
                    None => json!({}),
                },
                _ => json!({}),
            };
        }
        let text = super::message_text(taken);
        match self {
            Event::SessionStart { .. } => json!({ "additional_context": usage() }),
            Event::Stop { .. } => json!({ "followup_message": text }),
            Event::BeforeSubmitPrompt { .. } | Event::PostToolUse | Event::PostToolUseFailure => {
                json!({ "continue": true, "additional_context": text })
            }
            Event::PreToolUse { .. } => json!({
                "permission": "deny",
                "user_message": "loom message",
                "agent_message": text,
            }),
        }
    }
}

/// 포그라운드 Shell 명령 앞에 `loom guard` 줄을 붙인다. 중단이 오면 데몬이 그 셸을 끝낸다.
fn guarded(tool_name: &str, input: &Value) -> Option<Value> {
    if tool_name != "Shell" || input["is_background"] == true {
        return None;
    }
    let command = input["command"].as_str()?;
    let mut input = input.clone();
    input["command"] = format!("{GUARD}\n{command}").into();
    Some(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_trust_box_is_a_trust_wait() {
        // cursor-agent 2026.10.19가 신뢰하지 않은 폴더에서 그린 화면(실측).
        let screen = "│  Do you trust the contents of this directory?  │\n│  ▶ [a] Trust this workspace  │\n│    [q] Quit  │";
        assert_eq!(
            crate::screen::find(&DIALOGS, screen).map(|(kind, _)| kind),
            Some(WaitKind::Trust)
        );
    }

    /// cursor-agent 2026.10.19가 그린 질문 창과 명령 승인 창(실측).
    #[test]
    fn question_and_approval_boxes_are_found() {
        let question = "│ 색상 선택 │\n│ Question 1 of 1 │\n│ 1. Which color do you choose: red or blue? │\n│   › [ ] Red │\n│ ↑/↓ option · ←/→ question · Space select · Enter next/submit · Esc to skip │";
        assert_eq!(
            crate::screen::find(&DIALOGS, question).map(|(kind, _)| kind),
            Some(WaitKind::Question)
        );
        let approval = "  $ touch y.txt Waiting for approval...\n $  touch y.txt in .\n Run this command?\n Not in allowlist: touch\n  → Run (once) (y)";
        assert_eq!(
            crate::screen::find(&DIALOGS, approval).map(|(kind, _)| kind),
            Some(WaitKind::Approval)
        );
    }

    fn event(input: Value) -> Event {
        serde_json::from_value(input).unwrap()
    }

    fn delivery(kind: Kind) -> Delivery {
        Delivery {
            id: crate::graph::MessageId(7),
            from: "총괄".into(),
            kind,
            body: "go".into(),
            reply_to: None,
        }
    }

    /// 실측 입력(2026.10.19-6bbf2c4)에서 쓰는 필드만 읽는다.
    #[test]
    fn reads_measured_inputs() {
        let pre = event(json!({
            "conversation_id": "c", "model": "gpt-5.3-codex", "tool_name": "Shell",
            "tool_input": { "command": "echo hi", "cwd": "", "timeout": 30000 },
            "tool_use_id": "t", "hook_event_name": "preToolUse",
        }));
        assert_eq!(pre.take(), Some(Take::Now(&[Kind::Interrupt])));
        assert_eq!(pre.model(), None);
        let stop = event(json!({
            "hook_event_name": "stop", "status": "completed", "loop_count": 0, "model": "gpt-5.3-codex-low",
        }));
        assert_eq!(stop.take(), Some(Take::WhenIdle));
        assert_eq!(
            stop.model().and_then(|m| m.model).as_deref(),
            Some("gpt-5.3-codex-low")
        );
        assert_eq!(
            event(json!({ "hook_event_name": "postToolUse", "tool_output": "{}" })).take(),
            Some(Take::Now(&[Kind::Steer]))
        );
    }

    #[test]
    fn idle_mail_becomes_a_followup_and_interrupt_denies_the_tool() {
        let stop = event(json!({ "hook_event_name": "stop" }));
        let reply = stop.reply(&[delivery(Kind::Queue)]);
        assert!(reply["followup_message"].as_str().unwrap().contains("#7"));
        let pre = event(json!({
            "hook_event_name": "preToolUse", "tool_name": "Read", "tool_input": {},
        }));
        assert_eq!(
            pre.reply(&[delivery(Kind::Interrupt)])["permission"],
            "deny"
        );
    }

    #[test]
    fn foreground_shell_gets_the_guard_line() {
        let pre = event(json!({
            "hook_event_name": "preToolUse", "tool_name": "Shell",
            "tool_input": { "command": "npm test", "timeout": 30000 },
        }));
        assert_eq!(
            pre.reply(&[])["updated_input"],
            json!({ "command": "loom guard\nnpm test", "timeout": 30000 })
        );
        let background = event(json!({
            "hook_event_name": "preToolUse", "tool_name": "Shell",
            "tool_input": { "command": "loom wait", "is_background": true },
        }));
        assert_eq!(background.reply(&[]), json!({}));
    }

    /// Cursor가 버리지 않게 sessionStart 글마다 상한 아래로 둔다.
    #[test]
    fn each_session_start_context_fits() {
        assert!(usage().chars().count() <= CONTEXT_LIMIT_CHARS);
        for position in [Position::Root, Position::Child] {
            let text = rules(position)["additional_context"]
                .as_str()
                .unwrap()
                .to_owned();
            assert!(text.chars().count() <= CONTEXT_LIMIT_CHARS, "{position:?}");
        }
    }

    #[test]
    fn model_and_effort_join_into_one_id() {
        let choice = |model: Option<&str>, effort: Option<&str>| super::super::ModelChoice {
            model: model.map(Into::into),
            effort: effort.map(Into::into),
        };
        assert_eq!(
            choice_args(&choice(Some("claude-opus-5-5"), Some("high"))).unwrap(),
            ["--model", "claude-opus-5-5-high"]
        );
        assert_eq!(choice_args(&choice(None, Some("high"))), None);
    }
}
