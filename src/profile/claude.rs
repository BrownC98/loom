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
/// 쉬는 노드의 비동기 훅 상한. 이 시간보다 오래 쉬면 다음 사람 입력까지 깨우지 못한다.
/// Claude는 상한을 밀리초 타이머로 잰다. JavaScript 타이머는 2^31-1 밀리초(약 24.8일)를 넘기면 바로 끝난다.
const IDLE_HOOK_TIMEOUT_SECS: u64 = 2_000_000;
/// 비동기 훅이 깨울 때 모델이 읽는 머리말과 터미널 한 줄 요약(Claude 기본값은 "Stop hook feedback").
const REWAKE_MESSAGE: &str = "Messages from loom:";
const REWAKE_SUMMARY: &str = "loom message";
/// 기록 파일(transcript)에서 마지막 응답을 찾는 끝부분의 크기. 응답 한 줄은 보통 수 KB다.
const TRANSCRIPT_TAIL_BYTES: u64 = 1024 * 1024;
/// Claude가 API 응답 없이 쓴 응답(오류·중단)의 모델 값. 실제 모델이 아니다.
const SYNTHETIC_MODEL: &str = "<synthetic>";
/// Bash 명령 앞에 붙이는 줄. 셸을 중단 대상으로 데몬에 알린다.
const GUARD: &str = "loom guard";

fn usage() -> String {
    format!(
        "{}\nWhen no independent work remains, end your turn. loom wakes you when a message arrives. The loom guard line before foreground Bash commands is inserted by loom; do not remove it or add it yourself.\n\n{}",
        super::COMMUNICATION_USAGE,
        crate::skills::always_on()
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

/// `--model`·`--effort` 시작 인자(Claude Code 2.1.304 도움말).
pub fn choice_args(choice: &super::ModelChoice) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(model) = &choice.model {
        args.extend(["--model".into(), model.clone()]);
    }
    if let Some(effort) = &choice.effort {
        args.extend(["--effort".into(), effort.clone()]);
    }
    args
}

/// `claude` 시작 인자. 사용자의 `~/.claude` 파일을 고치지 않으려고 훅을 `--settings`로만 넘긴다.
/// `plugin`은 loom이 싣는 스킬 플러그인 폴더다(`skills::Plugin`). 스킬은 `loom:<이름>`으로 뜬다.
pub fn args(plugin: &Path) -> Vec<String> {
    let hook = json!([{ "hooks": [{
        "type": "command",
        "command": "loom hook",
        "timeout": HOOK_TIMEOUT_SECS,
    }] }]);
    // 턴이 없는 동안 메시지를 기다리고, 오면 exit 2로 모델을 깨운다. 사용자가 입력하던 글은 그대로 남는다.
    let idle = json!([{ "hooks": [{
        "type": "command",
        "command": "loom hook",
        "asyncRewake": true,
        "timeout": IDLE_HOOK_TIMEOUT_SECS,
        "rewakeMessage": REWAKE_MESSAGE,
        "rewakeSummary": REWAKE_SUMMARY,
    }] }]);
    let settings = json!({
        "permissions": { "allow": ALLOWED },
        "hooks": {
            "SessionStart": idle,
            "UserPromptSubmit": hook,
            "PreToolUse": hook,
            "PostToolUse": hook,
            "PostToolUseFailure": hook,
            "Stop": idle,
        },
    });
    vec![
        "--append-system-prompt".into(),
        usage(),
        "--settings".into(),
        settings.to_string(),
        "--plugin-dir".into(),
        plugin.to_string_lossy().into_owned(),
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

/// 훅이 메시지를 꺼내는 때.
#[derive(Debug, PartialEq)]
pub enum Take {
    /// 턴 안의 지점. 이 방식의 메시지만 바로 꺼낸다.
    Now(&'static [Kind]),
    /// 턴이 없다(세션 시작, 턴 끝). 비동기 훅이 메시지가 올 때까지 기다린다.
    WhenIdle,
}

impl Event {
    /// 이 지점에서 꺼내는 방법. 데몬은 노드가 일하는지 쉬는지 추적하지 않는다. 어느 훅이 불렸는지가 그 상태다.
    pub fn take(&self) -> Take {
        match self {
            Event::SessionStart { .. } | Event::Stop => Take::WhenIdle,
            // 쉬는 중이던 노드의 다음 턴: 모두 넘긴다.
            Event::UserPromptSubmit => Take::Now(Kind::ALL),
            Event::PostToolUse => Take::Now(&[Kind::Steer]),
            Event::PreToolUse { .. } | Event::PostToolUseFailure => Take::Now(&[Kind::Interrupt]),
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
        match self {
            // 비동기 훅은 JSON을 읽지 않는다. 본문은 stderr로 넘긴다(`cli::hook`).
            Event::SessionStart { .. } | Event::Stop => None,
            Event::PreToolUse { .. } => Some(json!({ "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": text,
            } })),
            Event::UserPromptSubmit => Some(context("UserPromptSubmit", text)),
            Event::PostToolUse => Some(context("PostToolUse", text)),
            Event::PostToolUseFailure => Some(context("PostToolUseFailure", text)),
        }
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
        assert_eq!(pre.take(), Take::Now(&[Kind::Interrupt]));
        let stop = event(json!({ "hook_event_name": "Stop", "stop_hook_active": true }));
        assert_eq!(stop.take(), Take::WhenIdle);
        assert_eq!(
            event(json!({ "hook_event_name": "PostToolUse" })).take(),
            Take::Now(&[Kind::Steer])
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
                    "loom message #1 (from: 총괄, mode: interrupt. Stop current work and follow this body)\n멈춰라",
            } }))
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
                    "additionalContext": "loom message #1 (from: 총괄, mode: steer)\nb",
                } }))
            );
        }
    }

    #[test]
    fn session_start_reports_the_model_and_waits_for_mail() {
        let input = json!({
            "session_id": "17dde44e", "hook_event_name": "SessionStart",
            "source": "startup", "model": "claude-opus-5-5",
        });
        let start = event(input.clone());
        assert_eq!(start.take(), Take::WhenIdle);
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
        let args = args(Path::new("/p"));
        assert_eq!(args[2], "--settings");
        assert_eq!(args[4..], ["--plugin-dir", "/p"]);
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
        // 턴이 없는 지점만 비동기로 기다린다. 턴 안의 훅이 비동기면 거부·맥락을 넘기지 못한다.
        for (name, idle) in [
            ("SessionStart", true),
            ("Stop", true),
            ("UserPromptSubmit", false),
            ("PreToolUse", false),
        ] {
            let hook = &settings["hooks"][name][0]["hooks"][0];
            assert_eq!(hook["asyncRewake"] == true, idle, "{name}");
            assert!(hook["timeout"].as_u64().unwrap() * 1000 < i32::MAX as u64);
        }
        let allow = settings["permissions"]["allow"].as_array().unwrap();
        assert!(allow.contains(&json!(format!("Bash({GUARD})"))));
        assert!(allow.contains(&json!("Bash(loom wait)")));
        assert!(!allow.iter().any(|a| a.as_str().unwrap().contains("spawn")));
    }
}
