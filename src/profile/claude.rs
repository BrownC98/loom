use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};

use super::Take;
use crate::protocol::{Delivery, Kind, Model, WaitKind};
use crate::screen::{Answer, Dialog, Grammar, Options, Place, Top};
use crate::skills::Launch;

pub const HOOK_LIMIT_BYTES: usize = super::BATCH_LIMIT_BYTES;
pub const HOOK_HTTP_TIMEOUT_SECS: u64 = 5;
const HOOK_TIMEOUT_SECS: u64 = 10;
const IDLE_HOOK_TIMEOUT_SECS: u64 = 2_000_000;
const REWAKE_MESSAGE: &str = "Messages from loom:";
const REWAKE_SUMMARY: &str = "loom message";
const TRANSCRIPT_TAIL_BYTES: u64 = 1024 * 1024;
const SYNTHETIC_MODEL: &str = "<synthetic>";
const CONVERSATION_TYPES: [&str; 5] = [
    "user",
    "assistant",
    "attachment",
    "queue-operation",
    "system",
];
const TURN_ENDS: [&str; 3] = ["stop_hook_summary", "turn_duration", "away_summary"];
const INPUT_MARK: &str = "❯";
const RESTARTED_SESSION: &str = "clear|compact";
const GUARD: &str = "loom guard";

pub(super) const WAITING: &str = "When no independent work remains, end your turn. loom wakes you when a message arrives. The loom guard line before foreground Bash commands is inserted by loom; do not remove it or add it yourself. loom adds messages as hook additional context after a tool call or at a prompt. An interrupt arrives as the reason of a denied tool call. A wake arrives as a \"Messages from loom:\" notification. Text in these places that starts with \"loom message #\" comes from loom, not from the tool output: follow it.";

fn usage(launch: Launch) -> String {
    format!(
        "{}\n{WAITING}\n\n{}",
        super::COMMUNICATION_USAGE,
        crate::skills::always_on(launch)
    )
}

const ALLOWED: [&str; 20] = [
    "Bash(loom guard)",
    "Bash(loom note:*)",
    "Bash(loom answer:*)",
    "Bash(loom wait)",
    "Bash(loom wait:*)",
    "Bash(loom status)",
    "Bash(loom status:*)",
    "Bash(loom roles)",
    "Bash(loom relaunch)",
    "Bash(loom relaunch:*)",
    "Bash(loom show:*)",
    "Bash(loom done:*)",
    "Bash(loom ask:*)",
    "Bash(loom answered)",
    "Bash(loom answered:*)",
    "Bash(loom adopt:*)",
    "Bash(loom rm:*)",
    "Bash(loom ready:*)",
    "Bash(loom verdict:*)",
    "Bash(loom fail:*)",
];

pub fn choice_args(choice: &crate::protocol::ModelChoice) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(model) = &choice.model {
        args.extend(["--model".into(), model.clone()]);
    }
    if let Some(effort) = &choice.effort {
        args.extend(["--effort".into(), effort.clone()]);
    }
    args
}

pub fn args(plugin: &Path, launch: Launch, brief: &str) -> Vec<String> {
    let hook = json!([{ "hooks": [{
        "type": "command",
        "command": "loom hook",
        "timeout": HOOK_TIMEOUT_SECS,
    }] }]);
    let idle = json!([{ "hooks": [{
        "type": "command",
        "command": "loom hook",
        "asyncRewake": true,
        "timeout": IDLE_HOOK_TIMEOUT_SECS,
        "rewakeMessage": REWAKE_MESSAGE,
        "rewakeSummary": REWAKE_SUMMARY,
    }] }]);
    let session_start = if brief.is_empty() {
        idle.clone()
    } else {
        json!([{ "matcher": RESTARTED_SESSION, "hooks": idle[0]["hooks"] }])
    };
    let settings = json!({
        "permissions": { "allow": ALLOWED },
        "hooks": {
            "SessionStart": session_start,
            "UserPromptSubmit": hook,
            "PreToolUse": hook,
            "PostToolUse": hook,
            "PostToolUseFailure": hook,
            "Stop": idle,
            "StopFailure": idle,
            "Notification": [{ "matcher": HUMAN_NOTIFICATIONS, "hooks": [{
                "type": "command",
                "command": "loom hook",
                "timeout": HOOK_TIMEOUT_SECS,
            }] }],
        },
    });
    vec![
        "--append-system-prompt".into(),
        usage(launch),
        "--settings".into(),
        settings.to_string(),
        "--plugin-dir".into(),
        plugin.to_string_lossy().into_owned(),
    ]
}

pub const PROMPT: Grammar = Grammar {
    top: Top::PanelEdge,
    options: Options::Numbered,
    guard: Some(GUARD),
};

pub const DIALOGS: [Dialog; 6] = [
    Dialog {
        text: "Choose the text style that looks best with your terminal",
        kind: WaitKind::Auth,
        place: Place::BeforeSession,
        answer: Answer::Below(Options::Listed),
    },
    Dialog {
        text: "Yes, I trust this folder",
        kind: WaitKind::Trust,
        place: Place::BeforeSession,
        answer: Answer::Panel(Options::Listed),
    },
    Dialog {
        text: "Yes, I accept",
        kind: WaitKind::Approval,
        place: Place::BeforeSession,
        answer: Answer::Panel(Options::Listed),
    },
    Dialog {
        text: "Select login method:",
        kind: WaitKind::Auth,
        place: Place::BeforeSession,
        answer: Answer::Below(Options::Numbered),
    },
    Dialog {
        text: "Allow external CLAUDE.md file imports?",
        kind: WaitKind::Trust,
        place: Place::BeforeSession,
        answer: Answer::Panel(Options::Listed),
    },
    Dialog {
        text: "Enter to confirm · Esc to cancel",
        kind: WaitKind::Approval,
        place: Place::OtherBeforeSession,
        answer: Answer::Panel(Options::Listed),
    },
];

const HUMAN_NOTIFICATIONS: &str = "permission_prompt|elicitation_dialog";
const ASK_TOOL: &str = "AskUserQuestion";
const PLAN_TOOL: &str = "ExitPlanMode";

#[derive(Deserialize)]
pub struct Hook {
    #[serde(flatten)]
    pub event: Event,
    agent_id: Option<String>,
}

impl Hook {
    pub fn take(&self) -> Take {
        match self.agent_id {
            Some(_) => Take::Never,
            None => self.event.take(),
        }
    }

    pub fn asks_interrupt(&self) -> bool {
        self.agent_id.is_some() && matches!(self.event, Event::PreToolUse { .. })
    }

    pub fn reports_wait(&self) -> bool {
        self.agent_id.is_none() || self.event.human_wait().is_some()
    }
}

const SUBAGENT_INTERRUPTED: &str =
    "Your parent agent received a loom interrupt. Stop now and return to your parent agent.";

pub fn subagent_interrupted() -> Value {
    deny(SUBAGENT_INTERRUPTED.into())
}

fn deny(text: String) -> Value {
    json!({ "hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "deny",
        "permissionDecisionReason": text,
    } })
}

#[derive(Deserialize)]
#[serde(tag = "hook_event_name")]
pub enum Event {
    SessionStart {
        model: Option<String>,
    },
    UserPromptSubmit {
        #[serde(default)]
        prompt: String,
    },
    PreToolUse {
        tool_name: String,
        tool_input: Value,
    },
    PostToolUse,
    PostToolUseFailure,
    Stop,
    StopFailure {
        #[serde(default)]
        error: String,
    },
    Notification {
        #[serde(default)]
        notification_type: String,
        #[serde(default)]
        message: String,
    },
}

const NOTIFICATION_TAG: &str = "<task-notification>";
const RATE_LIMIT_ERROR: &str = "rate_limit";
const RATE_LIMIT_TEXT: &str = "Stop and wait for limit to reset";

impl Event {
    pub fn take(&self) -> Take {
        match self {
            Event::SessionStart { .. } | Event::Stop | Event::StopFailure { .. } => Take::WhenIdle,
            Event::UserPromptSubmit { .. } => Take::Now(Kind::ALL),
            Event::PostToolUse => Take::Now(Kind::ALL),
            Event::PreToolUse { .. } | Event::PostToolUseFailure => Take::Now(&[Kind::Interrupt]),
            Event::Notification { .. } => Take::Never,
        }
    }

    pub fn human_input(&self) -> bool {
        self.prompt().is_some()
    }

    pub fn prompt(&self) -> Option<&str> {
        match self {
            Event::UserPromptSubmit { prompt } if !prompt.starts_with(NOTIFICATION_TAG) => {
                Some(prompt)
            }
            _ => None,
        }
    }

    pub fn human_wait(&self) -> Option<(WaitKind, String)> {
        match self {
            Event::PreToolUse {
                tool_name,
                tool_input,
            } if tool_name == ASK_TOOL => {
                let question = tool_input["questions"][0]["question"].as_str();
                Some((WaitKind::Question, question.unwrap_or_default().to_owned()))
            }
            Event::PreToolUse { tool_name, .. } if tool_name == PLAN_TOOL => {
                Some((WaitKind::Approval, String::new()))
            }
            Event::Notification {
                notification_type,
                message,
            } => {
                let kind = match notification_type.as_str() {
                    "elicitation_dialog" => WaitKind::Question,
                    _ => WaitKind::Approval,
                };
                Some((kind, message.clone()))
            }
            event if event.rate_limited() => Some((WaitKind::Auth, RATE_LIMIT_TEXT.to_owned())),
            _ => None,
        }
    }

    pub fn rate_limited(&self) -> bool {
        matches!(self, Event::StopFailure { error } if error == RATE_LIMIT_ERROR)
    }

    pub fn reply(&self, taken: &[Delivery], note: Option<&str>) -> Option<Value> {
        let Some(text) = super::hook_text(taken, note) else {
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
        };
        match self {
            Event::SessionStart { .. }
            | Event::Stop
            | Event::StopFailure { .. }
            | Event::Notification { .. } => None,
            Event::PreToolUse { .. } => Some(deny(text)),
            Event::UserPromptSubmit { .. } => Some(context("UserPromptSubmit", text)),
            Event::PostToolUse => Some(context("PostToolUse", text)),
            Event::PostToolUseFailure => Some(context("PostToolUseFailure", text)),
        }
    }
}

pub fn reported_model(event: &Event, input: &Value) -> Option<Model> {
    if let Event::SessionStart { model, .. } = event {
        return Some(Model {
            model: model.clone(),
            effort: None,
        });
    }
    let path = PathBuf::from(input["transcript_path"].as_str()?);
    last_model(&transcript_tail(&path)?)
}

pub fn last_reply(input: &Value) -> Option<String> {
    input["last_assistant_message"].as_str().map(str::to_owned)
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

pub fn turn_ended(transcript: &Path) -> bool {
    transcript_tail(transcript).is_some_and(|tail| ends_turn(&tail))
}

fn ends_turn(tail: &str) -> bool {
    #[derive(Deserialize)]
    struct Entry {
        r#type: String,
        subtype: Option<String>,
    }
    tail.lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Entry>(line).ok())
        .find(|e| CONVERSATION_TYPES.contains(&e.r#type.as_str()))
        .is_some_and(|e| {
            e.r#type == "system" && e.subtype.is_some_and(|s| TURN_ENDS.contains(&s.as_str()))
        })
}

pub fn input(screen: &str) -> Option<&str> {
    let lines: Vec<&str> = screen.lines().map(str::trim_end).collect();
    lines.windows(3).rev().find_map(|w| match w {
        [above, input, below] if is_rule(above) && is_rule(below) => {
            input.strip_prefix(INPUT_MARK).map(str::trim)
        }
        _ => None,
    })
}

fn is_rule(line: &str) -> bool {
    !line.is_empty() && line.chars().all(|c| c == '─')
}

fn context(event: &str, text: String) -> Value {
    json!({ "hookSpecificOutput": { "hookEventName": event, "additionalContext": text } })
}

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
    use crate::protocol::PromptKey;
    use crate::screen::Session;

    const APPROVAL: &str = concat!(
        "\n",
        "❯ Run exactly this shell command with the Bash tool: touch zz_file\n",
        "\n",
        "● Creating empty file zz_file\n",
        "  ⎿  $ touch zz_file\n",
        "\n",
        "────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────\n",
        " Bash command\n",
        " Tip: auto mode handles these prompts for you — choose \"switch to auto mode\" below\n",
        " Create empty file zz_file\n",
        "╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌\n",
        " touch zz_file\n",
        "╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌\n",
        " Do you want to proceed?\n",
        " ❯ 1. Yes\n",
        "   2. Yes, and always allow access to\n",
        "      /tmp/zz/repo\n",
        "      from this project\n",
        "   3. Yes, and switch to auto mode · auto mode handles these prompts for you\n",
        "   4. No\n",
        "\n",
        " Esc to cancel · Tab to amend\n",
    );

    #[test]
    fn approval_prompt_is_read_from_the_screen() {
        let prompt = crate::screen::prompt(&[PROMPT], &DIALOGS, APPROVAL, 120).unwrap();
        assert_eq!(
            prompt.body,
            [
                "Bash command",
                "Create empty file zz_file",
                "touch zz_file",
                "Do you want to proceed?"
            ]
        );
        assert_eq!(prompt.summary, "touch zz_file");
        let options: Vec<_> = prompt
            .options
            .iter()
            .map(|o| (o.label.as_str(), o.key.clone()))
            .collect();
        assert_eq!(
            options,
            [
                ("Yes", PromptKey::Char('1')),
                (
                    "Yes, and always allow access to /tmp/zz/repo from this project",
                    PromptKey::Char('2')
                ),
                (
                    "Yes, and switch to auto mode · auto mode handles these prompts for you",
                    PromptKey::Char('3')
                ),
                ("No", PromptKey::Char('4')),
            ]
        );
    }

    const NARROW: &str = concat!(
        "\n",
        "\n",
        "● Reporting completion to loom\n",
        "  ⎿  $ loom done \"Created zz_claude_2 with touch.\" --asked \"Run touch \n",
        "     zz_claude_2\"\n",
        "\n",
        "────────────────────────────────────────────────────────────────────────────────\n",
        " Bash command\n",
        " Tip: auto mode handles these prompts for you — choose \"switch to auto mode\" \n",
        " below\n",
        " Create empty file zz_claude_2\n",
        "╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌\n",
        " │ loom guard\n",
        " │ touch zz_claude_2\n",
        "╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌\n",
        " Do you want to proceed?\n",
        " ❯ 1. Yes\n",
        "   2. Yes, and always allow access to /tmp/claude-1000/-home-person-loom-workspa\n",
        "      ces-loom-------/c6d2fe08-623a-4ead-9013-09821a3da981/scratchpad/trepo from\n",
        "      this project\n",
        "   3. Yes, and switch to auto mode · auto mode handles these prompts for you\n",
        "   4. No\n",
        "\n",
        " Esc to cancel · Tab to amend\n",
    );

    #[test]
    fn wrapped_lines_and_the_guard_line_are_joined_or_dropped() {
        let prompt = crate::screen::prompt(&[PROMPT], &DIALOGS, NARROW, 80).unwrap();
        assert_eq!(
            prompt.body,
            [
                "Bash command",
                "Create empty file zz_claude_2",
                "touch zz_claude_2",
                "Do you want to proceed?"
            ]
        );
        assert_eq!(prompt.summary, "touch zz_claude_2");
        assert_eq!(
            prompt.options[1].label,
            "Yes, and always allow access to /tmp/claude-1000/-home-person-loom-workspaces-loom-------/c6d2fe08-623a-4ead-9013-09821a3da981/scratchpad/trepo from this project"
        );
    }

    #[test]
    fn command_rows_stay_whole_and_decide_the_identity() {
        let screen = |tail: &str| {
            APPROVAL.replace(
                " touch zz_file\n╌",
                &format!(
                    " │ echo ready\n │ Tip:; touch {tail}\n │ printf 'a\n │   b'\n │ cat <<EOF\n │ \tkeep\n │   indented\n │ $HOME\n │   keep\n │ EOF\n╌"
                ),
            )
        };
        let one = crate::screen::prompt(&[PROMPT], &DIALOGS, &screen("one"), 120).unwrap();
        assert_eq!(
            one.body,
            [
                "Bash command",
                "Create empty file zz_file",
                "echo ready",
                "Tip:; touch one",
                "printf 'a",
                "  b'",
                "cat <<EOF",
                "\tkeep",
                "  indented",
                "$HOME",
                "  keep",
                "EOF",
                "Do you want to proceed?"
            ]
        );
        assert_eq!(one.summary, "echo ready");
        let two = crate::screen::prompt(&[PROMPT], &DIALOGS, &screen("two"), 120).unwrap();
        assert_ne!(one, two);
        let spaced = screen("one").replace(" │   indented", " │    indented");
        assert_ne!(
            one,
            crate::screen::prompt(&[PROMPT], &DIALOGS, &spaced, 120).unwrap()
        );
    }

    #[test]
    fn numbered_lists_without_a_selection_are_not_prompts() {
        let answered = APPROVAL.replace(" ❯ 1. Yes", "   1. Yes");
        assert_eq!(
            crate::screen::prompt(&[PROMPT], &DIALOGS, &answered, 120),
            None
        );
        let list = "──────\n❯ plan\n● Steps:\n  1. Build\n  2. Test\n";
        assert_eq!(crate::screen::prompt(&[PROMPT], &DIALOGS, list, 80), None);
    }

    fn options(screen: &str) -> Vec<(String, PromptKey)> {
        crate::screen::prompt(&[PROMPT], &DIALOGS, screen, 100)
            .unwrap()
            .options
            .into_iter()
            .map(|o| (o.label, o.key))
            .collect()
    }

    #[test]
    fn first_dialogs_have_buttons() {
        let trust = include_str!("screens/claude-trust.txt");
        let prompt = crate::screen::prompt(&[PROMPT], &DIALOGS, trust, 100).unwrap();
        assert_eq!(prompt.body[..2], ["Accessing workspace:", "/tmp/repo"]);
        assert_eq!(
            options(trust),
            [
                ("No, exit".into(), PromptKey::Move(0)),
                ("Yes, I trust this folder".into(), PromptKey::Move(1)),
            ]
        );
        assert_eq!(PromptKey::Move(1).bytes().unwrap(), b"\x1b[B\r");
        let theme = options(include_str!("screens/claude-theme.txt"));
        assert_eq!(theme.len(), 7);
        assert_eq!(
            theme[0],
            ("Auto (match terminal)".into(), PromptKey::Move(-1))
        );
        assert_eq!(theme[1], ("Dark mode".into(), PromptKey::Move(0)));
        assert_eq!(
            options(include_str!("screens/claude-login.txt")),
            [
                (
                    "Claude account with subscription · Pro, Max, Team, or Enterprise".into(),
                    PromptKey::Char('1')
                ),
                (
                    "Anthropic Console account · API usage billing".into(),
                    PromptKey::Char('2')
                ),
                (
                    "3rd-party platform · Amazon Bedrock, Microsoft Foundry, Google Vertex AI"
                        .into(),
                    PromptKey::Char('3')
                ),
            ]
        );
    }

    #[test]
    fn a_startup_dialog_without_a_name_still_waits() {
        let imports = include_str!("screens/claude-imports.txt");
        let trust = include_str!("screens/claude-trust.txt");
        let unknown = imports.replace("Allow external CLAUDE.md file imports?", "New question?");
        let find = |screen| crate::screen::find(&DIALOGS, screen, Session::NotStarted);
        assert_eq!(
            find(imports),
            Some((
                WaitKind::Trust,
                "Allow external CLAUDE.md file imports?".into()
            ))
        );
        assert_eq!(
            find(trust),
            Some((WaitKind::Trust, "Yes, I trust this folder".into()))
        );
        assert_eq!(
            find(&unknown),
            Some((
                WaitKind::Approval,
                "Enter to confirm · Esc to cancel".into()
            ))
        );
        assert_eq!(
            crate::screen::find(&DIALOGS, &unknown, Session::Started),
            None
        );
        assert_eq!(
            options(&unknown),
            [
                ("No, disable external imports".into(), PromptKey::Move(0)),
                ("Yes, allow external imports".into(), PromptKey::Move(1)),
            ]
        );
    }

    #[test]
    fn the_input_box_is_not_a_list() {
        let idle = "──────\n❯ Try \"how do I log an error?\"\n──────\n  ⏵⏵ auto mode on (shift+tab to cycle)\n";
        assert_eq!(crate::screen::prompt(&[PROMPT], &DIALOGS, idle, 80), None);
    }

    #[test]
    fn launch_text_says_where_loom_mail_arrives() {
        let text = usage(root_launch());
        for place in [
            "hook additional context",
            "denied tool call",
            REWAKE_MESSAGE,
        ] {
            assert!(text.contains(place), "{place}");
        }
        let taken = [delivery(Kind::Steer, "b")];
        let reply = super::super::hook_text(&taken, None).unwrap();
        let start = "loom message #";
        assert!(reply.starts_with(start) && text.contains(&format!("starts with \"{start}\"")));
    }

    #[test]
    fn launch_text_has_the_ask_rules() {
        super::super::assert_every_launch_asks(|launch| (usage(launch), usage(launch)));
    }

    fn event(input: Value) -> Event {
        serde_json::from_value(input).unwrap()
    }

    fn delivery(kind: Kind, body: &str) -> Delivery {
        Delivery {
            id: crate::protocol::MessageId(1),
            from: "총괄".into(),
            kind,
            body: body.into(),
            what: Some(crate::protocol::MessageKind::Note),
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
            Take::Now(Kind::ALL)
        );
    }

    fn hook(input: Value) -> Hook {
        serde_json::from_value(input).unwrap()
    }

    #[test]
    fn subagent_hooks_take_no_mail_and_keep_the_guard_line() {
        let subagent = json!({
            "session_id": "e6edf215", "transcript_path": "/t.jsonl", "cwd": "/repo",
            "prompt_id": "0c124062", "permission_mode": "bypassPermissions",
            "agent_id": "a7432afa802847021", "agent_type": "general-purpose",
            "effort": { "level": "medium" },
        });
        let with = |fields: Value| {
            let mut input = subagent.clone();
            input
                .as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            hook(input)
        };
        let pre = with(json!({
            "hook_event_name": "PreToolUse", "tool_name": "Bash",
            "tool_input": { "command": "ls", "description": "List files" },
            "tool_use_id": "toolu_01",
        }));
        assert_eq!(pre.take(), Take::Never);
        assert_eq!(
            pre.event.reply(&[], None).unwrap()["hookSpecificOutput"]["updatedInput"]["command"],
            "loom guard\nls"
        );
        for fields in [
            json!({ "hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_response": {}, "duration_ms": 19 }),
            json!({ "hook_event_name": "PostToolUseFailure", "tool_name": "Bash" }),
            json!({ "hook_event_name": "UserPromptSubmit", "prompt": "x" }),
        ] {
            let other = with(fields.clone());
            assert_eq!(other.take(), Take::Never, "{fields}");
            assert_eq!(other.event.reply(&[], None), None, "{fields}");
        }
        let main = hook(json!({ "hook_event_name": "PostToolUse", "session_id": "e6edf215" }));
        assert_eq!(main.take(), Take::Now(Kind::ALL));
    }

    #[test]
    fn only_a_subagent_tool_call_asks_for_a_pending_interrupt_and_is_denied() {
        let pre = json!({
            "hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": { "command": "ls" },
        });
        let mut subagent = pre.clone();
        subagent["agent_id"] = "a7432afa802847021".into();
        assert!(hook(subagent.clone()).asks_interrupt());
        assert!(!hook(pre).asks_interrupt());
        subagent["hook_event_name"] = "PostToolUse".into();
        assert!(!hook(subagent).asks_interrupt());
        assert_eq!(
            subagent_interrupted(),
            json!({ "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": SUBAGENT_INTERRUPTED,
            } })
        );
    }

    #[test]
    fn a_subagent_reports_only_a_human_wait_it_raises() {
        let bash = json!({
            "hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": { "command": "ls" },
        });
        let ask = json!({
            "hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion",
            "tool_input": { "questions": [{ "question": "A or B?" }] },
        });
        assert!(hook(bash.clone()).reports_wait());
        for (mut input, reports) in [(bash, false), (ask, true)] {
            input["agent_id"] = "a7432afa802847021".into();
            assert_eq!(hook(input.clone()).reports_wait(), reports, "{input}");
        }
    }

    #[test]
    fn foreground_bash_gets_the_guard_line_and_keeps_other_fields() {
        let pre = event(json!({
            "hook_event_name": "PreToolUse", "tool_name": "Bash",
            "tool_input": { "command": "npm test && echo ok", "timeout": 1000 },
        }));
        assert_eq!(
            pre.reply(&[], None),
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
        assert_eq!(background.reply(&[], None), None);
        let read = event(json!({
            "hook_event_name": "PreToolUse", "tool_name": "Read",
            "tool_input": { "file_path": "/a" },
        }));
        assert_eq!(read.reply(&[], None), None);
    }

    #[test]
    fn interrupt_denies_the_next_tool_call() {
        let pre = event(json!({
            "hook_event_name": "PreToolUse", "tool_name": "Bash",
            "tool_input": { "command": "npm test" },
        }));
        assert_eq!(
            pre.reply(&[delivery(Kind::Interrupt, "멈춰라")], None),
            Some(json!({ "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason":
                    "loom message #1 (from: 총괄, kind: note, mode: interrupt. Stop your current work and do what this message says)\n멈춰라",
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
                event(input).reply(&[delivery(Kind::Steer, "b")], None),
                Some(json!({ "hookSpecificOutput": {
                    "hookEventName": name,
                    "additionalContext": "loom message #1 (from: 총괄, kind: note, mode: steer)\nb",
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

    #[test]
    fn stop_failure_waits_for_mail_and_raises_a_rate_limit_wait() {
        let limited = event(json!({ "hook_event_name": "StopFailure", "error": "rate_limit" }));
        assert_eq!(limited.take(), Take::WhenIdle);
        assert!(limited.rate_limited());
        assert_eq!(
            limited.human_wait(),
            Some((WaitKind::Auth, RATE_LIMIT_TEXT.into()))
        );
        let overloaded = event(json!({ "hook_event_name": "StopFailure", "error": "overloaded" }));
        assert!(!overloaded.rate_limited());
        assert!(!event(json!({ "hook_event_name": "Stop" })).rate_limited());
    }

    #[test]
    fn task_notifications_are_not_human_input() {
        let prompt =
            |text: &str| event(json!({ "hook_event_name": "UserPromptSubmit", "prompt": text }));
        assert!(prompt("Reply only HELLO.").human_input());
        assert!(!prompt("<task-notification>\n<task-id>b68lj1bk9</task-id>").human_input());
        assert!(
            !prompt("<task-notification>\n<summary>Stop hook feedback</summary>\n</task-notification>\n<system-reminder>")
                .human_input()
        );
        assert!(!event(json!({ "hook_event_name": "Stop" })).human_input());
    }

    #[test]
    fn human_input_gets_the_note_as_context() {
        let prompt = event(json!({ "hook_event_name": "UserPromptSubmit" }));
        assert_eq!(prompt.reply(&[], None), None);
        let reply = prompt.reply(&[], Some("note")).unwrap();
        assert_eq!(reply["hookSpecificOutput"]["additionalContext"], "note");
    }

    #[test]
    fn last_reply_is_the_stop_input_text() {
        let stop = json!({ "hook_event_name": "Stop", "last_assistant_message": "병합할까요?" });
        assert_eq!(last_reply(&stop).as_deref(), Some("병합할까요?"));
        assert_eq!(last_reply(&json!({ "hook_event_name": "Stop" })), None);
    }

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
        assert_eq!(last_model("ssistant\"}}\n{\"type\":\"user\"}\n"), None);
    }

    #[test]
    fn only_a_last_turn_end_record_ends_the_turn() {
        let ended = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"끝"}]}}
{"parentUuid":"a","isSidechain":false,"type":"system","subtype":"stop_hook_summary","hookCount":1,"hookInfos":[{"command":"loom hook"}]}
{"parentUuid":"b","isSidechain":false,"type":"system","subtype":"turn_duration","durationMs":15169,"messageCount":33}
{"type":"last-prompt","lastPrompt":"x"}
{"type":"ai-title","aiTitle":"x"}
{"type":"mode","mode":"normal"}
{"type":"permission-mode","permissionMode":"bypassPermissions"}
{"type":"atis-latch"}
"#;
        assert!(ends_turn(ended));
        let away = format!(
            "{ended}{}\n",
            r#"{"type":"system","subtype":"away_summary"}"#
        );
        assert!(ends_turn(&away));
        let tool = r#"{"type":"system","subtype":"turn_duration"}
{"type":"user","message":{"role":"user","content":"go"}}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash"}]}}
{"type":"api-request","x":1}
"#;
        assert!(!ends_turn(tool));
        let queued = format!(
            "{ended}{}\n",
            r#"{"type":"queue-operation","operation":"enqueue","content":"<task-notification>"}"#
        );
        assert!(!ends_turn(&queued));
        let local = format!(
            "{ended}{}\n",
            r#"{"type":"system","subtype":"local_command"}"#
        );
        assert!(!ends_turn(&local));
        assert!(!ends_turn("on\"}\n"));
        assert!(!ends_turn(""));
    }

    #[test]
    fn transcript_file_tells_a_turn_end() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"system\",\"subtype\":\"turn_duration\"}\n",
        )
        .unwrap();
        assert!(turn_ended(&path));
        assert!(!turn_ended(&dir.path().join("없음")));
    }

    #[test]
    fn the_input_box_gives_its_one_line() {
        let screen = |input: &str| {
            format!(
                "● Waiting for the next instruction.\n\n✻ Churned for 1s · done 9:48 AM\n\n────────\n{input}\n────────\n  ⏵⏵ bypass permissions on (shift+tab to cycle)"
            )
        };
        assert_eq!(input(&screen("❯ ")), Some(""));
        assert_eq!(input(&screen("❯")), Some(""));
        assert_eq!(input(&screen("❯ 고쳐 줘")), Some("고쳐 줘"));
        assert_eq!(input("──────\n❯ 첫 줄\n  둘째 줄\n──────\n"), None);
        assert_eq!(input(" ❯ 1. Yes\n   2. No\n"), None);
        assert_eq!(input(""), None);
    }

    #[test]
    fn a_task_notification_is_not_human_input() {
        let line = format!("{NOTIFICATION_TAG}{REWAKE_SUMMARY}</task-notification>");
        let submit = event(json!({ "hook_event_name": "UserPromptSubmit", "prompt": line }));
        assert!(!submit.human_input());
        assert_eq!(submit.take(), Take::Now(Kind::ALL));
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
    fn question_plan_and_permission_hooks_report_a_human_wait() {
        let ask = event(json!({
            "hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion",
            "tool_input": { "questions": [{ "question": "A or B?", "options": [] }] },
        }));
        assert_eq!(
            ask.human_wait(),
            Some((WaitKind::Question, "A or B?".into()))
        );
        let plan = event(json!({
            "hook_event_name": "PreToolUse", "tool_name": "ExitPlanMode", "tool_input": {},
        }));
        assert_eq!(plan.human_wait(), Some((WaitKind::Approval, String::new())));
        let permission = event(json!({
            "hook_event_name": "Notification", "notification_type": "permission_prompt",
            "message": "Claude needs your permission to use Bash",
        }));
        assert_eq!(permission.take(), Take::Never);
        assert_eq!(
            permission.human_wait(),
            Some((
                WaitKind::Approval,
                "Claude needs your permission to use Bash".into()
            ))
        );
        let elicitation = event(json!({
            "hook_event_name": "Notification", "notification_type": "elicitation_dialog",
            "message": "Server asks",
        }));
        assert_eq!(elicitation.human_wait().unwrap().0, WaitKind::Question);
    }

    #[test]
    fn hooks_after_the_dialog_clear_the_wait() {
        for input in [
            json!({ "hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": { "command": "ls" } }),
            json!({ "hook_event_name": "PostToolUse" }),
            json!({ "hook_event_name": "PostToolUseFailure" }),
            json!({ "hook_event_name": "UserPromptSubmit" }),
            json!({ "hook_event_name": "Stop" }),
            json!({ "hook_event_name": "StopFailure", "error": "overloaded" }),
        ] {
            assert_eq!(event(input.clone()).human_wait(), None, "{input}");
        }
    }

    #[test]
    fn settings_register_the_eight_hooks_and_allow_messaging_but_not_spawn() {
        let args = args(Path::new("/p"), root_launch(), "");
        assert_eq!(args[2], "--settings");
        assert_eq!(args[4..], ["--plugin-dir", "/p"]);
        let settings: Value = serde_json::from_str(&args[3]).unwrap();
        let mut events: Vec<_> = settings["hooks"].as_object().unwrap().keys().collect();
        events.sort();
        assert_eq!(
            events,
            [
                "Notification",
                "PostToolUse",
                "PostToolUseFailure",
                "PreToolUse",
                "SessionStart",
                "Stop",
                "StopFailure",
                "UserPromptSubmit"
            ]
        );
        for (name, idle) in [
            ("SessionStart", true),
            ("Stop", true),
            ("StopFailure", true),
            ("UserPromptSubmit", false),
            ("PreToolUse", false),
        ] {
            let hook = &settings["hooks"][name][0]["hooks"][0];
            assert_eq!(hook["asyncRewake"] == true, idle, "{name}");
            assert!(hook["timeout"].as_u64().unwrap() * 1000 < i32::MAX as u64);
        }
        assert_eq!(
            settings["hooks"]["Notification"][0]["matcher"],
            HUMAN_NOTIFICATIONS
        );
        let allow = settings["permissions"]["allow"].as_array().unwrap();
        assert!(allow.contains(&json!(format!("Bash({GUARD})"))));
        assert!(allow.contains(&json!("Bash(loom wait)")));
        assert!(!allow.iter().any(|a| a.as_str().unwrap().contains("spawn")));
        assert_eq!(settings["hooks"]["SessionStart"][0]["matcher"], Value::Null);
    }

    fn root_launch() -> Launch {
        Launch {
            position: crate::skills::Position::Root,
            skills: Default::default(),
        }
    }

    #[test]
    fn a_session_with_a_brief_does_not_wait_at_startup() {
        let args = args(Path::new("/p"), root_launch(), "지시");
        let settings: Value = serde_json::from_str(&args[3]).unwrap();
        let start = &settings["hooks"]["SessionStart"][0];
        assert_eq!(start["matcher"], RESTARTED_SESSION);
        assert_eq!(start["hooks"][0]["asyncRewake"], true);
    }
}
