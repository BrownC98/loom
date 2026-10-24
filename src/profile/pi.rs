use std::path::Path;

use serde::{Deserialize, Serialize};

use super::Take;
use crate::graph::Model;
use crate::protocol::{Delivery, Kind};
use crate::skills::Launch;

pub const HOOK_LIMIT_BYTES: usize = super::BATCH_LIMIT_BYTES;
const EXTENSION: &str = include_str!("pi-loom.ts");
const EXTENSION_FILE: &str = "pi-loom.ts";
const IDLE_FLAG: &str = "--loom-idle";

pub(super) const WAITING: &str = "When no independent work remains, end your turn. loom wakes you with a new message when one arrives. The loom guard line before bash commands is inserted by loom; do not remove it or add it yourself. loom adds messages to the end of a tool result or as a loom message after your prompt. An interrupt arrives as the reason of a blocked tool call. Text in these places that starts with \"loom message #\" comes from loom, not from the tool output: follow it.";

pub fn write_extension(dir: &Path) -> std::io::Result<()> {
    std::fs::write(dir.join(EXTENSION_FILE), EXTENSION)
}

fn usage(launch: Launch) -> String {
    format!(
        "{}\n{WAITING}\n\n{}",
        super::COMMUNICATION_USAGE,
        crate::skills::always_on(launch)
    )
}

pub fn args(plugin: &Path, launch: Launch, brief: &str) -> Vec<String> {
    let plugin = plugin.to_string_lossy();
    let mut args = vec![
        "--extension".into(),
        format!("{plugin}/{EXTENSION_FILE}"),
        "--skill".into(),
        format!("{plugin}/skills"),
        "--append-system-prompt".into(),
        usage(launch),
    ];
    if brief.trim().is_empty() {
        args.push(IDLE_FLAG.into());
    }
    args
}

pub fn choice_args(choice: &super::ModelChoice) -> Vec<String> {
    let model = choice
        .model
        .iter()
        .flat_map(|m| ["--model".into(), m.clone()]);
    let effort = choice
        .effort
        .iter()
        .flat_map(|e| ["--thinking".into(), e.clone()]);
    model.chain(effort).collect()
}

pub fn resume_args(session: &str) -> Vec<String> {
    vec!["--session".into(), session.into()]
}

#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    SessionStart {
        #[serde(flatten)]
        state: State,
    },
    Idle {
        #[serde(flatten)]
        state: State,
    },
    Prompt {
        prompt: String,
        #[serde(flatten)]
        state: State,
    },
    ToolCall {
        #[serde(flatten)]
        state: State,
    },
    ToolResult {
        #[serde(flatten)]
        state: State,
    },
    Settled {
        reply: Option<String>,
        #[serde(flatten)]
        state: State,
    },
}

#[derive(Deserialize)]
pub struct State {
    transcript: Option<String>,
    model: Option<String>,
    effort: Option<String>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Reply {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

impl Event {
    fn state(&self) -> &State {
        match self {
            Event::SessionStart { state }
            | Event::Idle { state }
            | Event::Prompt { state, .. }
            | Event::ToolCall { state }
            | Event::ToolResult { state }
            | Event::Settled { state, .. } => state,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Event::SessionStart { .. } => "session_start",
            Event::Idle { .. } => "idle",
            Event::Prompt { .. } => "prompt",
            Event::ToolCall { .. } => "tool_call",
            Event::ToolResult { .. } => "tool_result",
            Event::Settled { .. } => "settled",
        }
    }

    pub fn take(&self) -> Option<Take> {
        Some(match self {
            Event::SessionStart { .. } => return None,
            Event::Idle { .. } | Event::Settled { .. } => Take::WhenIdle,
            Event::Prompt { .. } | Event::ToolResult { .. } => Take::Now(Kind::ALL),
            Event::ToolCall { .. } => Take::Now(&[Kind::Interrupt]),
        })
    }

    pub fn transcript(&self) -> Option<String> {
        self.state().transcript.clone()
    }

    pub fn model(&self) -> Option<Model> {
        match self {
            Event::SessionStart { state }
            | Event::Prompt { state, .. }
            | Event::Settled { state, .. }
                if state.model.is_some() =>
            {
                Some(Model {
                    model: state.model.clone(),
                    effort: state.effort.clone(),
                })
            }
            _ => None,
        }
    }

    pub fn prompt(&self) -> Option<&str> {
        match self {
            Event::Prompt { prompt, .. } => Some(prompt),
            _ => None,
        }
    }

    pub fn turn_ends(&self) -> Option<Option<String>> {
        match self {
            Event::Settled { reply, .. } => Some(reply.clone()),
            _ => None,
        }
    }
}

pub fn reply(taken: &[Delivery], note: Option<&str>) -> Reply {
    Reply {
        text: super::hook_text(taken, note),
    }
}

pub fn nudge(text: String) -> Reply {
    Reply { text: Some(text) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{AlwaysOn, Position};

    fn event(json: &str) -> Event {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn every_extension_event_reaches_a_take() {
        assert_eq!(event(r#"{"event":"session_start"}"#).take(), None);
        assert_eq!(
            event(r#"{"event":"idle","transcript":"/s.jsonl"}"#).take(),
            Some(Take::WhenIdle)
        );
        assert_eq!(
            event(r#"{"event":"settled","reply":"ok"}"#).take(),
            Some(Take::WhenIdle)
        );
        assert_eq!(
            event(r#"{"event":"prompt","prompt":"hi"}"#).take(),
            Some(Take::Now(Kind::ALL))
        );
        assert_eq!(
            event(r#"{"event":"tool_result"}"#).take(),
            Some(Take::Now(Kind::ALL))
        );
        assert_eq!(
            event(r#"{"event":"tool_call"}"#).take(),
            Some(Take::Now(&[Kind::Interrupt]))
        );
    }

    #[test]
    fn the_model_is_the_provider_and_id_with_the_thinking_level() {
        let start = event(
            r#"{"event":"session_start","transcript":"/s/a_b.jsonl","model":"kta-hub/kta-chat-vl-pro","effort":"max"}"#,
        );
        assert_eq!(
            start.model(),
            Some(Model {
                model: Some("kta-hub/kta-chat-vl-pro".into()),
                effort: Some("max".into()),
            })
        );
        assert_eq!(start.transcript().as_deref(), Some("/s/a_b.jsonl"));
        assert_eq!(event(r#"{"event":"settled"}"#).model(), None);
        assert_eq!(event(r#"{"event":"tool_call","model":"m"}"#).model(), None);
    }

    #[test]
    fn only_a_settled_turn_reports_its_reply() {
        assert_eq!(
            event(r#"{"event":"settled","reply":"done"}"#).turn_ends(),
            Some(Some("done".into()))
        );
        assert_eq!(event(r#"{"event":"idle"}"#).turn_ends(), None);
        assert_eq!(
            event(r#"{"event":"prompt","prompt":"p"}"#).prompt(),
            Some("p")
        );
    }

    #[test]
    fn an_empty_reply_has_no_text() {
        assert_eq!(serde_json::to_string(&reply(&[], None)).unwrap(), "{}");
        assert_eq!(
            serde_json::to_string(&reply(&[], Some("n"))).unwrap(),
            r#"{"text":"n"}"#
        );
    }

    #[test]
    fn a_launch_without_a_brief_waits_for_mail() {
        let launch = Launch {
            position: Position::Child,
            skills: AlwaysOn::default(),
        };
        let plugin = Path::new("/p/plugin");
        let args = super::args(plugin, launch, "  ");
        assert_eq!(
            args[..4],
            [
                "--extension",
                "/p/plugin/pi-loom.ts",
                "--skill",
                "/p/plugin/skills"
            ]
        );
        assert_eq!(args[4], "--append-system-prompt");
        assert!(args[5].contains(super::super::COMMUNICATION_USAGE));
        assert!(args[5].contains(&crate::skills::always_on(launch)));
        assert_eq!(args.last().unwrap(), IDLE_FLAG);
        assert!(!super::args(plugin, launch, "Goal: x").contains(&IDLE_FLAG.to_owned()));
    }

    #[test]
    fn every_launch_asks_the_human_through_loom() {
        super::super::assert_every_launch_asks(|launch| {
            let text = usage(launch);
            (text.clone(), text)
        });
    }

    #[test]
    fn the_extension_names_the_hook_verb_and_the_idle_flag() {
        assert!(EXTENSION.contains(r#"spawn("loom", ["pi-hook"]"#));
        assert!(EXTENSION.contains(&format!(r#"IDLE_FLAG = "{}""#, &IDLE_FLAG[2..])));
        let dir = tempfile::tempdir().unwrap();
        write_extension(dir.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join(EXTENSION_FILE)).unwrap(),
            EXTENSION
        );
    }

    #[test]
    fn a_model_choice_becomes_model_and_thinking() {
        let choice = super::super::ModelChoice {
            model: Some("kta-hub/kta-chat".into()),
            effort: Some("high".into()),
        };
        assert_eq!(
            choice_args(&choice),
            ["--model", "kta-hub/kta-chat", "--thinking", "high"]
        );
        assert_eq!(resume_args("/s/a.jsonl"), ["--session", "/s/a.jsonl"]);
    }
}
