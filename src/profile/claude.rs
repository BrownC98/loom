use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};

use super::{Handed, HookEvent, Report, Shown, Signals, Take};
use crate::protocol::{Delivery, Kind, Model, WaitKind};
use crate::screen::{Answer, Dialog, Grammar, Options, Place, Top};
use crate::skills::Launch;

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
    transcript_path: Option<String>,
    last_assistant_message: Option<String>,
}

impl HookEvent for Hook {
    fn signals(&self) -> Signals {
        let main = self.agent_id.is_none();
        let wait = self.event.human_wait();
        Signals {
            hook: self.event.name().to_owned(),
            model: reported_model(&self.event, self.transcript_path.as_deref()),
            take: self.event.take().filter(|_| main),
            transcript: self.transcript_path.clone(),
            prompt: self.event.prompt().map(str::to_owned),
            human_input: self.event.prompt().is_some(),
            turn_end: matches!(self.event, Event::Stop)
                .then(|| self.last_assistant_message.clone()),
            rate_limited: self.event.rate_limited(),
            asks_interrupt: !main && matches!(self.event, Event::PreToolUse { .. }),
            reports: (main || wait.is_some())
                .then_some(Report::Wait(wait))
                .into_iter()
                .collect(),
            ..Signals::default()
        }
    }

    fn show(&self, handed: &Handed) -> Shown {
        match handed {
            Handed::Taken { messages, note } => Shown {
                out: self
                    .event
                    .reply(messages, note.as_deref())
                    .map(|reply| reply.to_string()),
                ..Shown::default()
            },
            Handed::Interrupted => Shown {
                out: Some(deny(SUBAGENT_INTERRUPTED.into()).to_string()),
                ..Shown::default()
            },
            Handed::Mail(messages) if messages.is_empty() => Shown::default(),
            Handed::Mail(messages) => to_model(super::message_text(messages)),
            Handed::Nudge(text) => to_model(text.clone()),
        }
    }
}

fn to_model(text: String) -> Shown {
    Shown {
        out: None,
        to_model: Some(text),
        rewake: true,
    }
}

const SUBAGENT_INTERRUPTED: &str =
    "Your parent agent received a loom interrupt. Stop now and return to your parent agent.";

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
    fn name(&self) -> &'static str {
        match self {
            Event::SessionStart { .. } => "SessionStart",
            Event::UserPromptSubmit { .. } => "UserPromptSubmit",
            Event::PreToolUse { .. } => "PreToolUse",
            Event::PostToolUse => "PostToolUse",
            Event::PostToolUseFailure => "PostToolUseFailure",
            Event::Stop => "Stop",
            Event::StopFailure { .. } => "StopFailure",
            Event::Notification { .. } => "Notification",
        }
    }

    pub fn take(&self) -> Option<Take> {
        Some(match self {
            Event::SessionStart { .. } | Event::Stop | Event::StopFailure { .. } => Take::WhenIdle,
            Event::UserPromptSubmit { .. } => Take::Now(Kind::ALL),
            Event::PostToolUse => Take::Now(Kind::ALL),
            Event::PreToolUse { .. } | Event::PostToolUseFailure => Take::Now(&[Kind::Interrupt]),
            Event::Notification { .. } => return None,
        })
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

fn reported_model(event: &Event, transcript: Option<&str>) -> Option<Model> {
    if let Event::SessionStart { model, .. } = event {
        return Some(Model {
            model: model.clone(),
            effort: None,
        });
    }
    last_model(&transcript_tail(&PathBuf::from(transcript?))?)
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
