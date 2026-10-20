//! Claude Code 프로필: 시작 인자(사용법 지시, 훅 설정)와 훅 입출력 형식.
//! 바뀌는 이유는 Claude Code 버전이다. 형식은 https://code.claude.com/docs/en/hooks 를 따른다.
//! 근거: docs/design.md 「에이전트 실행」, 「전달 지점」.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::graph::{Delivery, Kind, Model};

/// 훅 한 번에 넘기는 본문 합의 상한.
pub const HOOK_LIMIT_BYTES: usize = super::BATCH_LIMIT_BYTES;
/// `loom hook`이 데몬 응답을 기다리는 상한. 훅 `timeout`(`HOOK_TIMEOUT_SECS`)보다 짧아야 한다.
/// 그래야 Claude가 훅을 먼저 끝내 꺼낸 본문을 버리는 일이 드물다. 버려도 받음 확인 전이므로 데몬이 다시 준다.
pub const HOOK_HTTP_TIMEOUT_SECS: u64 = 5;
const HOOK_TIMEOUT_SECS: u64 = 10;
/// 기록 파일(transcript)에서 마지막 응답을 찾는 끝부분의 크기. 응답 한 줄은 보통 수 KB다.
const TRANSCRIPT_TAIL_BYTES: u64 = 1024 * 1024;
/// Claude가 API 응답 없이 쓴 응답(오류·중단)의 모델 값. 실제 모델이 아니다.
const SYNTHETIC_MODEL: &str = "<synthetic>";
/// Bash 명령 앞에 붙이는 줄. 셸을 중단 대상으로 데몬에 알린다.
const GUARD: &str = "loom guard";

fn usage() -> String {
    format!(
        "{}\nWhen no independent work remains, run loom wait with Bash run_in_background and end your turn. Its completed output is the incoming message. The loom guard line before foreground Bash commands is inserted by loom; do not remove it or add it yourself.",
        super::COMMUNICATION_USAGE
    )
}

/// 승인 없이 부르는 명령. 기본 권한 모드에서 메시지마다 승인 창이 뜨면 사람이 없는 노드가 멈춘다.
/// `loom spawn`은 넣지 않는다. `-- <명령>`으로 아무 명령이나 띄울 수 있다.
const ALLOWED: [&str; 10] = [
    "Bash(loom guard)",
    "Bash(loom send:*)",
    "Bash(loom steer:*)",
    "Bash(loom interrupt:*)",
    "Bash(loom wait)",
    "Bash(loom wait:*)",
    "Bash(loom reply:*)",
    "Bash(loom status)",
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
            "SessionStart": hook,
            "UserPromptSubmit": hook,
            "PreToolUse": hook,
            "PostToolUse": hook,
            "PostToolUseFailure": hook,
            "Stop": hook,
        },
    });
    vec![
        "--append-system-prompt".into(),
        usage(),
        "--settings".into(),
        settings.to_string(),
    ]
}

/// 훅 입력에서 loom이 보는 필드만 읽는다. 다른 필드는 버린다.
#[derive(Deserialize)]
#[serde(tag = "hook_event_name")]
pub enum Event {
    /// 세션 시작. 모델만 알린다(실측: 2.1.304 입력의 `model`).
    SessionStart {
        model: Option<String>,
    },
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
            // 아직 턴이 없다. 첫 턴은 UserPromptSubmit이 받는다.
            Event::SessionStart { .. } => &[],
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
            Event::SessionStart { .. } => context("SessionStart", text),
            Event::UserPromptSubmit => context("UserPromptSubmit", text),
            Event::PostToolUse => context("PostToolUse", text),
            Event::PostToolUseFailure => context("PostToolUseFailure", text),
        })
    }
}

/// 훅 입력이 알리는 모델과 추론 강도. 알리지 않는 훅이면 없다.
/// 세션 시작은 입력의 `model`을, 그 뒤 훅은 기록 파일의 마지막 응답을 쓴다. 훅 입력에는 `/model`로 바꾼 모델이 없다(실측).
pub fn reported_model(event: &Event, input: &Value) -> Option<Model> {
    if let Event::SessionStart { model } = event {
        return Some(Model {
            model: model.clone(),
            effort: None,
        });
    }
    let path = PathBuf::from(input["transcript_path"].as_str()?);
    last_model(&transcript_tail(&path)?)
}

fn transcript_tail(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TRANSCRIPT_TAIL_BYTES)))
        .ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    Some(String::from_utf8_lossy(&tail).into_owned())
}

/// 기록 파일 끝부분에서 본 대화의 마지막 응답이 쓴 모델과 추론 강도. 첫 줄은 잘렸을 수 있어 읽지 못하면 건너뛴다.
/// 하위 에이전트 응답(`isSidechain`)과 API 없이 쓴 응답은 건너뛴다.
fn last_model(tail: &str) -> Option<Model> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Entry {
        r#type: String,
        #[serde(default)]
        is_sidechain: bool,
        effort: Option<String>,
        message: Option<Message>,
    }
    #[derive(Deserialize)]
    struct Message {
        model: Option<String>,
    }
    tail.lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Entry>(line).ok())
        .filter(|e| e.r#type == "assistant" && !e.is_sidechain)
        .find_map(|e| {
            let model = e.message?.model.filter(|m| m != SYNTHETIC_MODEL)?;
            Some(Model {
                model: Some(model),
                effort: e.effort,
            })
        })
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
            reply_to: None,
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
    fn session_start_reports_the_model_and_takes_nothing() {
        let input = json!({
            "session_id": "17dde44e", "hook_event_name": "SessionStart",
            "source": "startup", "model": "claude-opus-5-5",
        });
        let start = event(input.clone());
        assert_eq!(start.kinds(), [] as [Kind; 0]);
        assert_eq!(
            reported_model(&start, &input),
            Some(Model {
                model: Some("claude-opus-5-5".into()),
                effort: None
            })
        );
    }

    /// 2.1.304 기록 파일에서 옮긴 줄이다(본문은 줄였다). `/model sonnet`, `/effort high` 뒤 응답이 마지막 본 대화 응답이다.
    #[test]
    fn transcript_gives_the_last_main_chain_reply() {
        let tail = r#"{"type":"assistant","isSidechain":false,"effort":"medium","message":{"model":"claude-opus-5-5","role":"assistant"}}
{"type":"user","isSidechain":false,"message":{"role":"user","content":"/model sonnet"}}
{"type":"assistant","isSidechain":false,"effort":"high","perTurnEffort":"high","requestedModel":"claude-sonnet-5-5","message":{"model":"claude-sonnet-5-5","role":"assistant"}}
{"type":"assistant","isSidechain":true,"effort":"low","message":{"model":"claude-haiku-5-5"}}
{"type":"assistant","isSidechain":false,"message":{"model":"<synthetic>"}}
{"type":"system","content":"x"}
"#;
        assert_eq!(
            last_model(tail),
            Some(Model {
                model: Some("claude-sonnet-5-5".into()),
                effort: Some("high".into())
            })
        );
        // 잘린 첫 줄과 응답 없는 기록은 모델이 없다.
        assert_eq!(last_model("ssistant\"}}\n{\"type\":\"user\"}\n"), None);
    }

    #[test]
    fn hooks_after_start_read_the_transcript_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        std::fs::write(
            &path,
            r#"{"type":"assistant","effort":"xhigh","message":{"model":"claude-opus-5-5"}}"#,
        )
        .unwrap();
        let input = json!({ "hook_event_name": "Stop", "transcript_path": path });
        assert_eq!(
            reported_model(&event(input.clone()), &input),
            Some(Model {
                model: Some("claude-opus-5-5".into()),
                effort: Some("xhigh".into())
            })
        );
        let missing =
            json!({ "hook_event_name": "Stop", "transcript_path": dir.path().join("없음") });
        assert_eq!(reported_model(&event(missing.clone()), &missing), None);
    }

    #[test]
    fn settings_register_the_six_hooks_and_allow_messaging_but_not_spawn() {
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
                "SessionStart",
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
