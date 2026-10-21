//! Cursor CLI 프로필: 시작 인자, 플러그인 훅 설정, 훅 입출력 형식.
//! 바뀌는 이유는 Cursor CLI 버전이다. 형식은 https://cursor.com/docs/agent/hooks 를 따른다.
//! 실측: cursor-agent 2026.10.19-6bbf2c4. 근거: docs/design.md 「에이전트 실행」, 「전달 지점」.

use std::path::Path;

use serde::Deserialize;
use serde_json::{Value, json};

use super::Take;
use crate::graph::{Delivery, Kind, Model, OpenQuestion, WaitKind};
use crate::screen::{Dialog, Place};
use crate::skills::{AlwaysOn, Launch, Position};

/// 훅이 알리지 않는 TUI 확인 창(2026.10.19 화면 문구). 폴더 신뢰·명령 승인·로그인·질문은 실측, 나머지는 번들 글이다.
/// "Run this command"는 샌드박스 밖 실행 창("Run this command outside the sandbox?")도 맞춘다.
/// 질문 도구(AskQuestion)는 preToolUse 훅을 부르지 않는다(실측). 그래서 질문도 화면으로 찾는다.
pub const DIALOGS: [Dialog; 8] = [
    Dialog {
        // 창 제목(예: "Clarifying Questions")은 모델이 정한다. 도움말 줄은 고정이다(실측).
        text: "Enter next/submit",
        kind: WaitKind::Question,
        place: Place::Panel,
    },
    Dialog {
        text: "Do you trust the contents of this directory?",
        kind: WaitKind::Trust,
        place: Place::BeforeSession,
    },
    Dialog {
        text: "Run this command",
        kind: WaitKind::Approval,
        place: Place::Panel,
    },
    Dialog {
        text: "Run this MCP tool?",
        kind: WaitKind::Approval,
        place: Place::Panel,
    },
    Dialog {
        text: "Read this file?",
        kind: WaitKind::Approval,
        place: Place::Panel,
    },
    Dialog {
        text: "Write to this file?",
        kind: WaitKind::Approval,
        place: Place::Panel,
    },
    Dialog {
        text: "Delete this file?",
        kind: WaitKind::Approval,
        place: Place::Panel,
    },
    Dialog {
        text: "Press any key to log in",
        kind: WaitKind::Auth,
        place: Place::BeforeSession,
    },
];

/// 노드 프로세스에 넣는 시작 자리. 훅은 모든 노드가 같은 파일을 쓰므로 자리를 환경 변수로 받는다.
pub const POSITION_ENV: &str = "LOOM_POSITION";
/// 노드 프로세스에 넣는 스킬 스위치(`AlwaysOn` JSON). 같은 이유로 환경 변수다.
pub const SKILLS_ENV: &str = "LOOM_SKILLS";
/// Cursor는 훅 `additional_context` 한 개가 이 글자 수를 넘으면 버린다(실측 번들의 상한).
/// 그래서 사용법과 상시 지시를 sessionStart 훅 둘로 나눈다. 부모 없는 노드의 상시 지시는 약 9,100자다(2026-10-21 실측).
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
            "afterAgentResponse": hook,
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

/// 스킬 스위치를 환경 변수 값으로.
pub fn skills_value(skills: AlwaysOn) -> String {
    serde_json::to_string(&skills).expect("스위치는 늘 직렬화된다")
}

/// 값이 없거나 읽지 못하면 모두 켜짐으로 본다. 이 변수 전에 뜬 노드가 그렇다.
pub fn skills_from(value: Option<&str>) -> AlwaysOn {
    value
        .and_then(|v| serde_json::from_str(v).ok())
        .unwrap_or_default()
}

/// 첫 sessionStart 훅의 글: 통신 명령과 Cursor 대기 방식.
pub fn usage() -> String {
    format!(
        "{}\nWhen no independent work remains, end your turn. loom wakes you with a new message when one arrives. Cursor runs that wake hook only when ~/.cursor/hooks.json has a \"stop\" entry; if it has none, wait for an expected reply with loom wait --reply-to <message-id> --timeout <seconds> before you end your turn. The loom guard line before shell commands is inserted by loom; do not remove it or add it yourself.",
        super::COMMUNICATION_USAGE
    )
}

/// 둘째 sessionStart 훅의 출력: 상시 지시.
pub fn rules(launch: Launch) -> Value {
    json!({ "additional_context": crate::skills::always_on(launch) })
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
    /// 모델이 답 글을 다 냈다. stop 훅보다 먼저 온다. 턴 끝 질문 검사가 쓴다(2026.10.19 번들의 입력 `text`).
    AfterAgentResponse {
        #[serde(default)]
        text: String,
    },
}

impl Event {
    /// 이 지점에서 꺼내는 방법. Claude와 같은 표다(`claude::Event::take`).
    /// sessionStart는 꺼내지 않는다. Cursor는 그 훅이 끝나야 첫 턴을 띄운다. 쉬는 동안의 메시지는 첫 stop 훅이 넘긴다.
    pub fn take(&self) -> Option<Take> {
        Some(match self {
            Event::SessionStart { .. } | Event::AfterAgentResponse { .. } => return None,
            Event::Stop { .. } => Take::WhenIdle,
            Event::BeforeSubmitPrompt { .. } => Take::Now(Kind::ALL),
            Event::PostToolUse => Take::BetweenTools,
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
    pub fn reply(
        &self,
        taken: &[Delivery],
        queued_from: &[String],
        open_question: Option<&OpenQuestion>,
    ) -> Value {
        let Some(text) = super::hook_text(taken, queued_from, open_question) else {
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
        };
        match self {
            Event::SessionStart { .. } => json!({ "additional_context": usage() }),
            Event::Stop { .. } => json!({ "followup_message": text }),
            Event::BeforeSubmitPrompt { .. } | Event::PostToolUse | Event::PostToolUseFailure => {
                json!({ "continue": true, "additional_context": text })
            }
            Event::AfterAgentResponse { .. } => json!({}),
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
    use crate::screen::Session;

    #[test]
    fn the_trust_box_is_a_trust_wait() {
        // cursor-agent 2026.10.19가 신뢰하지 않은 폴더에서 그린 화면(실측). sessionStart 훅은 신뢰한 뒤에 불린다(실측).
        let screen = "│  Do you trust the contents of this directory?  │\n│  ▶ [a] Trust this workspace  │\n│    [q] Quit  │";
        assert_eq!(
            crate::screen::find(&DIALOGS, screen, Session::NotStarted).map(|(kind, _)| kind),
            Some(WaitKind::Trust)
        );
        assert_eq!(
            crate::screen::find(&DIALOGS, screen, Session::Started),
            None
        );
    }

    /// cursor-agent 2026.10.19가 100×30 터미널에 그린 질문 창(실측, 가로를 줄였다).
    const QUESTION: &str = "  Use your ask-question tool to ask me one multiple-choice question: red or blue.\n\n  I'll ask that now.\n\n ┌──────────────────┐\n │ Clarifying Questions │\n │ Question 1 of 1 │\n │ 1. Red or blue? │\n │   › [ ] Red │\n │     [ ] Blue │\n │ ↑/↓ option · ←/→ question · Space select · Enter next/submit · Esc to skip │\n └──────────────────┘\n";
    /// 같은 버전의 명령 승인 창(실측). 창 위에 가로줄이 있다.
    const APPROVAL: &str = "  I'll create the directory and stop there.\n\n  $ mkdir zz_dir Waiting for approval...\n\n──────────────────────\n $  mkdir zz_dir in .\n\n Run this command?\n Not in allowlist: mkdir\n  → Run (once) (y)\n    Add Shell(mkdir) to allowlist? (tab)\n    Run Everything (shift+tab)\n    Skip & tell the agent what to do instead (esc or n)\n\n\n";
    /// 같은 버전이 쉴 때의 화면 아래(실측). 답의 마지막 줄 아래에 입력 칸과 상태 줄만 있다.
    fn idle_after(answer: &str) -> String {
        format!(
            "  $ touch y.txt 558ms\n\n  {answer}\n\n\n  → Add a follow-up\n\n\n  Claude Sonnet 5 300K High  [----------] 7%  cur @ master\n"
        )
    }

    #[test]
    fn question_and_approval_boxes_are_found_in_a_session() {
        assert_eq!(
            crate::screen::find(&DIALOGS, QUESTION, Session::Started).map(|(kind, _)| kind),
            Some(WaitKind::Question)
        );
        assert_eq!(
            crate::screen::find(&DIALOGS, APPROVAL, Session::Started).map(|(kind, _)| kind),
            Some(WaitKind::Approval)
        );
    }

    /// 에이전트가 창 글을 출력해도 창이 아니다(2026-10-21 Claude 노드의 거짓 대기와 같은 경우).
    #[test]
    fn dialog_text_in_agent_output_is_not_a_box() {
        for text in [
            "Run this command?",
            "Enter next/submit",
            "Do you trust the contents of this directory?",
        ] {
            let middle = idle_after(&format!("{text}\n  Done."));
            assert_eq!(
                crate::screen::find(&DIALOGS, &middle, Session::Started),
                None,
                "{text}"
            );
            let last = idle_after(text);
            assert_eq!(
                crate::screen::find(&DIALOGS, &last, Session::Started),
                None,
                "{text}"
            );
        }
    }

    /// 에이전트가 가로줄과 표를 출력한 뒤 창 글을 쓴 화면(2026.10.19 실측, 2026-10-21 다시 찍음, 가로를 줄였다).
    /// 대화 기록은 2칸 들여 그린다. 창 테두리는 왼쪽 끝에 그린다(위 `QUESTION`, `APPROVAL`).
    const PRINTED: &str = "  Reply with exactly this text.\n\n\n  ┌───┬───┐\n  │ A │ B │\n  ├───┼───┤\n  │ 1 │ 2 │\n  └───┴───┘\n\n  ─────\n  Run this command?\n  Enter next/submit\n  done\n\n\n\n\n  → Add a follow-up\n\n\n  Auto Balance Balance  [----------] 5%  cur @ master\n";
    /// 같은 버전이 에이전트가 그린 표 아래에 띄운 질문 창(2026-10-21 실측, 가로를 줄였다).
    const QUESTION_BELOW_TABLE: &str = "  ┌───┬───┐\n  │ A │ B │\n  ├───┼───┤\n  │ 1 │ 2 │\n  └───┴───┘\n\n ┌──────────────────┐\n │ Clarifying Questions │\n │ Question 1 of 1 │\n │ 1. Red or blue? │\n │   › [ ] red │\n │     [ ] blue │\n │ ↑/↓ option · ←/→ question · Space select · Enter next/submit · Esc to skip │\n └──────────────────┘\n";

    /// 에이전트가 출력한 가로줄·상자 안이나 아래의 창 글은 창이 아니다. 창 테두리만 왼쪽 끝에 있다.
    #[test]
    fn dialog_text_below_a_printed_rule_or_box_is_not_a_box() {
        let in_box = idle_after(
            "┌───────────────────┐\n  │ Run this command? │\n  │ Enter next/submit │\n  └───────────────────┘",
        );
        let working = PRINTED.replace("→ Add a follow-up", "⠠⠛ Working\n\n  → Add a follow-up");
        for screen in [PRINTED, &in_box, &working] {
            assert_eq!(
                crate::screen::find(&DIALOGS, screen, Session::Started),
                None,
                "{screen}"
            );
        }
        assert_eq!(
            crate::screen::find(&DIALOGS, QUESTION_BELOW_TABLE, Session::Started),
            Some((WaitKind::Question, "Enter next/submit".into()))
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
            Some(Take::BetweenTools)
        );
    }

    #[test]
    fn idle_mail_becomes_a_followup_and_interrupt_denies_the_tool() {
        let stop = event(json!({ "hook_event_name": "stop" }));
        let reply = stop.reply(&[delivery(Kind::Queue)], &[], None);
        assert!(reply["followup_message"].as_str().unwrap().contains("#7"));
        let pre = event(json!({
            "hook_event_name": "preToolUse", "tool_name": "Read", "tool_input": {},
        }));
        assert_eq!(
            pre.reply(&[delivery(Kind::Interrupt)], &[], None)["permission"],
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
            pre.reply(&[], &[], None)["updated_input"],
            json!({ "command": "loom guard\nnpm test", "timeout": 30000 })
        );
        let background = event(json!({
            "hook_event_name": "preToolUse", "tool_name": "Shell",
            "tool_input": { "command": "loom wait", "is_background": true },
        }));
        assert_eq!(background.reply(&[], &[], None), json!({}));
    }

    /// Cursor가 버리지 않게 sessionStart 글마다 상한 아래로 둔다.
    /// 답 글 훅은 메시지를 꺼내지 않고 답 글만 넘긴다(`cli::cursor_hook`). 출력은 빈 JSON이다.
    #[test]
    fn after_agent_response_carries_the_reply_text() {
        let said = event(json!({ "hook_event_name": "afterAgentResponse", "text": "병합할까요?" }));
        assert!(matches!(&said, Event::AfterAgentResponse { text } if text == "병합할까요?"));
        assert_eq!(said.take(), None);
        assert_eq!(said.reply(&[], &[], None), json!({}));
        assert!(hooks()["hooks"]["afterAgentResponse"].is_array());
    }

    #[test]
    fn session_start_contexts_have_the_ask_rules() {
        super::super::assert_every_launch_asks(|launch| {
            let rules = rules(launch)["additional_context"]
                .as_str()
                .unwrap()
                .to_owned();
            (usage(), rules)
        });
    }

    #[test]
    fn each_session_start_context_fits() {
        assert!(usage().chars().count() <= CONTEXT_LIMIT_CHARS);
        for position in [Position::Root, Position::Child] {
            let launch = Launch {
                position,
                skills: AlwaysOn::default(),
            };
            let text = rules(launch)["additional_context"]
                .as_str()
                .unwrap()
                .to_owned();
            assert!(text.chars().count() <= CONTEXT_LIMIT_CHARS, "{position:?}");
        }
    }

    /// 이 변수 전에 뜬 노드(값 없음)는 모든 스킬을 받는다. 데몬이 쓴 값은 그대로 읽는다.
    #[test]
    fn skills_env_round_trips_and_defaults_to_all_on() {
        assert_eq!(skills_from(None), AlwaysOn::default());
        assert_eq!(skills_from(Some("x")), AlwaysOn::default());
        let off = AlwaysOn {
            o_mode: false,
            ..AlwaysOn::default()
        };
        assert_eq!(skills_from(Some(&skills_value(off))), off);
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
