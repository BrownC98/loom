//! Claude Code 프로필: 시작 인자(사용법 지시, 훅 설정)와 훅 입출력 형식.
//! 바뀌는 이유는 Claude Code 버전이다. 형식은 https://code.claude.com/docs/en/hooks 를 따른다.
//! 근거: docs/design.md 「에이전트 실행」, 「전달 지점」.

use serde::Deserialize;
use serde_json::{Value, json};

use crate::graph::{Delivery, Kind};

/// 훅 한 번에 넘기는 본문 합의 상한.
pub const HOOK_LIMIT_BYTES: usize = super::BATCH_LIMIT_BYTES;
/// `loom hook`이 데몬 응답을 기다리는 상한. 훅 `timeout`(`HOOK_TIMEOUT_SECS`)보다 짧아야 한다.
/// 그래야 Claude가 훅을 먼저 끝내 꺼낸 본문을 버리는 일이 드물다. 버려도 받음 확인 전이므로 데몬이 다시 준다.
pub const HOOK_HTTP_TIMEOUT_SECS: u64 = 5;
const HOOK_TIMEOUT_SECS: u64 = 10;
/// Bash 명령 앞에 붙이는 줄. 셸을 중단 대상으로 데몬에 알린다.
const GUARD: &str = "loom guard";

const USAGE: &str = "\
너는 loom 그래프의 노드다. loom은 부모·자식 노드 사이 메시지를 나른다. 명령은 Bash로 부른다.
- `loom spawn <이름> --agent claude` (지시문은 stdin, `--file <경로>`): 자식 노드를 만든다.
- `loom send <이름> <본문>`: 부모나 자식에게 보낸다. 받는 노드의 턴을 끊지 않는다.
- `loom steer <이름> <본문>`: 일하는 자식의 다음 도구 호출 사이에 넣는다.
- `loom interrupt <이름> <본문>`: 자식이 하던 일을 멈추게 하고 이 본문을 따르게 한다.
- `loom wait`: 메시지 하나를 기다렸다가 출력한다.
- `loom done <요약>` / `loom done --file <경로>`: 결과를 부모에게 보내고 끝낸다. 요약은 1KB, 본문은 4KB 이하다.
- `loom adopt <이름>`: 그 노드의 새 부모가 된다. 그 노드가 부모 없이 보관한 메시지를 받는다.
받은 메시지는 「loom 메시지」로 시작하는 맥락으로 들어온다. 중단 메시지를 받으면 하던 일을 멈추고 그 본문을 따른다.
자식의 결과를 기다리는 동안 할 일이 없으면 Bash `run_in_background`로 `loom wait`를 띄우고 턴을 끝낸다. 그 명령이 끝나면 출력이 받은 메시지다.
Bash 명령 앞의 `loom guard` 줄은 loom이 붙인다. 지우거나 직접 쓰지 않는다.";

/// 승인 없이 부르는 명령. 기본 권한 모드에서 메시지마다 승인 창이 뜨면 사람이 없는 노드가 멈춘다.
/// `loom spawn`은 넣지 않는다. `-- <명령>`으로 아무 명령이나 띄울 수 있다.
const ALLOWED: [&str; 7] = [
    "Bash(loom guard)",
    "Bash(loom send:*)",
    "Bash(loom steer:*)",
    "Bash(loom interrupt:*)",
    "Bash(loom wait)",
    "Bash(loom done:*)",
    "Bash(loom adopt:*)",
];

/// `claude` 시작 인자. 사용자의 `~/.claude` 파일을 고치지 않으려고 훅을 `--settings`로만 넘긴다.
pub fn args() -> Vec<String> {
    let hook = json!([{ "hooks": [{
        "type": "command",
        "command": "loom hook",
        "timeout": HOOK_TIMEOUT_SECS,
    }] }]);
    let settings = json!({
        "permissions": { "allow": ALLOWED },
        "hooks": {
            "UserPromptSubmit": hook,
            "PreToolUse": hook,
            "PostToolUse": hook,
            "PostToolUseFailure": hook,
            "Stop": hook,
        },
    });
    vec![
        "--append-system-prompt".into(),
        USAGE.into(),
        "--settings".into(),
        settings.to_string(),
    ]
}

/// 훅 입력에서 loom이 보는 필드만 읽는다. 다른 필드는 버린다.
#[derive(Deserialize)]
#[serde(tag = "hook_event_name")]
pub enum Event {
    UserPromptSubmit,
    PreToolUse {
        tool_name: String,
        tool_input: Value,
    },
    PostToolUse,
    PostToolUseFailure,
    Stop,
}

impl Event {
    /// 이 지점에서 꺼내는 전달 방식. 데몬은 노드가 일하는지 쉬는지 추적하지 않는다. 어느 훅이 불렸는지가 그 상태다.
    pub fn kinds(&self) -> &'static [Kind] {
        match self {
            // 쉬는 중이던 노드의 다음 턴, 일하던 노드의 턴 끝: 모두 넘긴다.
            Event::UserPromptSubmit | Event::Stop => Kind::ALL,
            Event::PostToolUse => &[Kind::Steer],
            Event::PreToolUse { .. } | Event::PostToolUseFailure => &[Kind::Interrupt],
        }
    }

    /// 훅 출력 JSON. 없으면 아무것도 출력하지 않는다.
    pub fn reply(&self, taken: &[Delivery]) -> Option<Value> {
        if taken.is_empty() {
            return match self {
                Event::PreToolUse {
                    tool_name,
                    tool_input,
                } => guarded(tool_name, tool_input).map(|input| {
                    json!({ "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "updatedInput": input,
                    } })
                }),
                _ => None,
            };
        }
        let text = super::message_text(taken);
        Some(match self {
            Event::Stop => json!({ "decision": "block", "reason": text }),
            Event::PreToolUse { .. } => json!({ "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": text,
            } }),
            Event::UserPromptSubmit => context("UserPromptSubmit", text),
            Event::PostToolUse => context("PostToolUse", text),
            Event::PostToolUseFailure => context("PostToolUseFailure", text),
        })
    }
}

fn context(event: &str, text: String) -> Value {
    json!({ "hookSpecificOutput": { "hookEventName": event, "additionalContext": text } })
}

/// 포그라운드 Bash 명령 앞에 `loom guard` 줄을 붙인다. 중단이 오면 데몬이 그 셸을 끝낸다.
/// 백그라운드 명령은 붙이지 않는다. 결과를 기다리는 백그라운드 `loom wait`를 중단이 끝내면 안 된다.
/// 권한 규칙은 바꾼 명령으로 판정된다. 줄을 이어 붙이면 Claude가 줄마다 판정하므로 사용자의 규칙이 그대로 맞는다.
fn guarded(tool_name: &str, input: &Value) -> Option<Value> {
    if tool_name != "Bash" || input["run_in_background"] == true {
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

    fn event(input: Value) -> Event {
        serde_json::from_value(input).unwrap()
    }

    fn delivery(kind: Kind, body: &str) -> Delivery {
        Delivery {
            id: crate::graph::MessageId(1),
            from: "총괄".into(),
            kind,
            body: body.into(),
        }
    }

    #[test]
    fn reads_only_the_fields_loom_uses() {
        let pre = event(json!({
            "session_id": "abc", "hook_event_name": "PreToolUse", "tool_name": "Bash",
            "tool_input": { "command": "npm test", "description": "테스트" }, "tool_use_id": "t1",
        }));
        assert!(matches!(&pre, Event::PreToolUse { tool_name, .. } if tool_name == "Bash"));
        assert_eq!(pre.kinds(), [Kind::Interrupt]);
        let stop = event(json!({ "hook_event_name": "Stop", "stop_hook_active": true }));
        assert_eq!(stop.kinds(), Kind::ALL);
        assert_eq!(
            event(json!({ "hook_event_name": "PostToolUse" })).kinds(),
            [Kind::Steer]
        );
    }

    #[test]
    fn foreground_bash_gets_the_guard_line_and_keeps_other_fields() {
        let pre = event(json!({
            "hook_event_name": "PreToolUse", "tool_name": "Bash",
            "tool_input": { "command": "npm test && echo ok", "timeout": 1000 },
        }));
        assert_eq!(
            pre.reply(&[]),
            Some(json!({ "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "updatedInput": { "command": "loom guard\nnpm test && echo ok", "timeout": 1000 },
            } }))
        );
    }

    #[test]
    fn background_bash_and_other_tools_are_left_alone() {
        let background = event(json!({
            "hook_event_name": "PreToolUse", "tool_name": "Bash",
            "tool_input": { "command": "loom wait", "run_in_background": true },
        }));
        assert_eq!(background.reply(&[]), None);
        let read = event(json!({
            "hook_event_name": "PreToolUse", "tool_name": "Read",
            "tool_input": { "file_path": "/a" },
        }));
        assert_eq!(read.reply(&[]), None);
    }

    #[test]
    fn interrupt_denies_the_next_tool_call() {
        let pre = event(json!({
            "hook_event_name": "PreToolUse", "tool_name": "Bash",
            "tool_input": { "command": "npm test" },
        }));
        assert_eq!(
            pre.reply(&[delivery(Kind::Interrupt, "멈춰라")]),
            Some(json!({ "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason":
                    "loom 메시지 #1 (보낸 노드: 총괄, 방식: 중단. 하던 일을 멈추고 이 본문을 따른다)\n멈춰라",
            } }))
        );
    }

    #[test]
    fn stop_blocks_with_every_message() {
        let stop = event(json!({ "hook_event_name": "Stop" }));
        assert_eq!(stop.reply(&[]), None);
        assert_eq!(
            stop.reply(&[delivery(Kind::Queue, "a"), delivery(Kind::Steer, "b")]),
            Some(json!({
                "decision": "block",
                "reason": "loom 메시지 #1 (보낸 노드: 총괄, 방식: 큐)\na\n\nloom 메시지 #1 (보낸 노드: 총괄, 방식: 조정)\nb",
            }))
        );
    }

    #[test]
    fn prompt_and_tool_results_get_context() {
        for (name, input) in [
            (
                "UserPromptSubmit",
                json!({ "hook_event_name": "UserPromptSubmit", "prompt": "x" }),
            ),
            ("PostToolUse", json!({ "hook_event_name": "PostToolUse" })),
            (
                "PostToolUseFailure",
                json!({ "hook_event_name": "PostToolUseFailure", "is_interrupt": false }),
            ),
        ] {
            assert_eq!(
                event(input).reply(&[delivery(Kind::Steer, "b")]),
                Some(json!({ "hookSpecificOutput": {
                    "hookEventName": name,
                    "additionalContext": "loom 메시지 #1 (보낸 노드: 총괄, 방식: 조정)\nb",
                } }))
            );
        }
    }

    #[test]
    fn settings_register_the_five_hooks_and_allow_messaging_but_not_spawn() {
        let args = args();
        assert_eq!(args[2], "--settings");
        let settings: Value = serde_json::from_str(&args[3]).unwrap();
        let mut events: Vec<_> = settings["hooks"].as_object().unwrap().keys().collect();
        events.sort();
        assert_eq!(
            events,
            [
                "PostToolUse",
                "PostToolUseFailure",
                "PreToolUse",
                "Stop",
                "UserPromptSubmit"
            ]
        );
        let allow = settings["permissions"]["allow"].as_array().unwrap();
        assert!(allow.contains(&json!(format!("Bash({GUARD})"))));
        assert!(allow.contains(&json!("Bash(loom wait)")));
        assert!(!allow.iter().any(|a| a.as_str().unwrap().contains("spawn")));
    }
}
