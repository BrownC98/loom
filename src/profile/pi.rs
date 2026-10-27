use std::path::Path;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};

use portable_pty::CommandBuilder;

use super::adapter::{
    Adapter, Carry, Chosen, Command, Facts, InputSeen, LaunchAt, Logins, Port, Record, ResumeError,
    Started,
};
use super::{Handed, HookEvent, Shown, Signals, Take, VersionMismatch};
use crate::accounts::Site;
use crate::protocol::{
    Delivery, Kind, LaunchError, MessageId, Model, ModelChoice, Prompt, Report, Session,
};
use crate::screen::{Dialog, Grammar, Screen};
use crate::skills::Launch;

const EXTENSION: &str = include_str!("pi-loom.ts");
const EXTENSION_FILE: &str = "pi-loom.ts";
const HOOK: &str = include_str!("loom-hook.ts");
const EXTENSIONS: [(&str, &str); 2] = [("loom-hook.ts", HOOK), (EXTENSION_FILE, EXTENSION)];
pub(super) const IDLE_FLAG: &str = "--loom-idle";
const UNNUMBERED: u32 = 2;
const PROTOCOL: u32 = 4;

pub(super) const WAITING: &str = "When no independent work remains, end your turn. loom wakes you with a new message when one arrives. The loom guard line before bash commands is inserted by loom; do not remove it or add it yourself. loom adds messages to the end of a tool result or as a loom message after your prompt. An interrupt arrives as the reason of a blocked tool call. Text in these places that starts with \"loom message #\" comes from loom, not from the tool output: follow it.";

pub(super) const WORK_METHOD: &str = "Work method (speed; follow it):
- Read a whole file, or a range of 400 lines or more, in one call, not 100-line pieces. Read related files in the same turn and collect searches into one command.
- Before the first build, write the plan of the whole edit (files, functions, tests) one time, then make all edits. Do not restate the brief or the plan in later turns.
- While you edit, run only the tests of the code you changed. Run the full checks of the repository one time, in one chained command, just before the commit. Do not run a check again when no code changed since it passed.
- In a review-fix round, run only the tests of the code you changed, and the full checks one time at the end of the round.
- Wait for a background job with the command or tool that waits for its end, not with a fixed sleep.
- Do not search the disk for tools; use the paths that the repository instructions or the brief give, or ask.";

pub struct Pi;

impl Adapter for Pi {
    fn facts(&self) -> Facts {
        Facts {
            logins: Logins::None,
            sleeps: true,
            listens_at_start: false,
            pinned: None,
            usage_part: "usage/pi",
            waiting: WAITING,
            helper_shells: &[],
        }
    }

    fn leading_args(&self, resume: Option<&Session>, bypass: &[String]) -> Vec<String> {
        resume
            .into_iter()
            .flat_map(|session| ["--session".into(), session.0.clone()])
            .chain(bypass.iter().cloned())
            .collect()
    }

    fn prepare(&self, _: &LaunchAt<'_>) -> std::io::Result<()> {
        Ok(())
    }

    fn command(&self, cmd: &mut CommandBuilder, at: Command<'_>) {
        cmd.args(args(at.plugin, at.launch, at.brief));
    }

    fn launch<'a>(
        &'a self,
        _: &'a LaunchAt<'_>,
        _: Option<&'a Session>,
        _: &'a [String],
        _: &'a mut CommandBuilder,
        _: Arc<dyn Port>,
    ) -> BoxFuture<'a, Result<Started, LaunchError>> {
        Box::pin(async { Ok(Started::default()) })
    }

    fn attach<'a>(&'a self, _: &'a Record, _: Arc<dyn Port>) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }

    fn answer<'a>(
        &'a self,
        _: &'a Record,
        _: &'a Prompt,
        _: usize,
    ) -> BoxFuture<'a, Result<Chosen, String>> {
        Box::pin(async { Ok(Chosen::OnScreen) })
    }

    fn choice_args(&self, choice: &ModelChoice) -> Option<Vec<String>> {
        Some(choice_args(choice))
    }

    fn screen(&self) -> Option<(&'static [Grammar], &'static [Dialog])> {
        Some((&[], &[]))
    }

    fn input_seen(&self, _: &Screen) -> Option<InputSeen> {
        None
    }

    fn install(&self, dir: &Path) -> std::io::Result<()> {
        EXTENSIONS
            .iter()
            .try_for_each(|(file, text)| std::fs::write(dir.join(file), text))
    }

    fn version_warning(&self, _: &str) -> Option<VersionMismatch> {
        None
    }

    fn carry(&self, _: &Session, _: &Carry<'_>) -> Result<(), ResumeError> {
        Ok(())
    }

    fn end_detached<'a>(&'a self, _: &'a Record, _: &'a Site) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }

    fn record<'a>(&'a self, rec: &'a Record) -> BoxFuture<'a, Option<String>> {
        Box::pin(async { rec.session.as_ref().map(|s| s.0.clone()) })
    }

    fn shell_root(&self, _: &Record) -> Option<u32> {
        None
    }
}

pub(super) fn usage(waiting: &str, launch: Launch) -> String {
    format!(
        "{}\n{waiting}\n\n{WORK_METHOD}\n\n{}",
        super::COMMUNICATION_USAGE,
        crate::skills::always_on(launch)
    )
}

fn args(plugin: &Path, launch: Launch, brief: &str) -> Vec<String> {
    let plugin = plugin.to_string_lossy();
    let mut args = vec![
        "--extension".into(),
        format!("{plugin}/{EXTENSION_FILE}"),
        "--no-skills".into(),
        "--skill".into(),
        format!("{plugin}/skills"),
        "--append-system-prompt".into(),
        usage(WAITING, launch),
    ];
    if brief.trim().is_empty() {
        args.push(IDLE_FLAG.into());
    }
    args
}

pub(super) fn choice_args(choice: &ModelChoice) -> Vec<String> {
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

pub struct Request {
    pub id: Option<u64>,
    pub event: Event,
}

pub struct Unserved {
    pub id: Option<u64>,
    pub error: String,
}

#[derive(Deserialize)]
struct Header {
    protocol: Option<u32>,
    id: Option<u64>,
}

pub fn request(line: &str) -> Result<Request, Unserved> {
    let header: Header = serde_json::from_str(line).map_err(|e| Unserved {
        id: None,
        error: format!("hook input: {e}"),
    })?;
    let id = header.id;
    let unserved = |error| Err(Unserved { id, error });
    match (header.protocol, id) {
        (Some(PROTOCOL), Some(_)) | (None, None) => {}
        (Some(PROTOCOL), None) => {
            return unserved(format!("a hook protocol {PROTOCOL} request has no id"));
        }
        (other, _) => {
            let other = other.unwrap_or(UNNUMBERED);
            return unserved(format!(
                "the pi extension speaks hook protocol {other}, but this loom speaks {PROTOCOL}"
            ));
        }
    }
    match serde_json::from_str(line) {
        Ok(event) => Ok(Request { id, event }),
        Err(e) => unserved(format!("hook input: {e}")),
    }
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
    Ack {
        ids: Vec<MessageId>,
        #[serde(flatten)]
        state: State,
    },
    Interrupt {
        #[serde(flatten)]
        state: State,
    },
    Close {
        of: u64,
        #[serde(flatten)]
        state: State,
    },
}

pub enum Served<'a> {
    Hook,
    Ack(&'a [MessageId]),
    Interrupt,
    Close(u64),
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
    id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    ids: Vec<MessageId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    interrupted: bool,
}

impl Event {
    fn state(&self) -> &State {
        match self {
            Event::SessionStart { state }
            | Event::Idle { state }
            | Event::Prompt { state, .. }
            | Event::ToolCall { state }
            | Event::ToolResult { state }
            | Event::Settled { state, .. }
            | Event::Ack { state, .. }
            | Event::Interrupt { state }
            | Event::Close { state, .. } => state,
        }
    }

    pub fn served(&self) -> Served<'_> {
        match self {
            Event::Ack { ids, .. } => Served::Ack(ids),
            Event::Interrupt { .. } => Served::Interrupt,
            Event::Close { of, .. } => Served::Close(*of),
            _ => Served::Hook,
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
            Event::Ack { .. } => "ack",
            Event::Interrupt { .. } => "interrupt",
            Event::Close { .. } => "close",
        }
    }

    pub fn take(&self) -> Option<Take> {
        Some(match self {
            Event::SessionStart { .. }
            | Event::Ack { .. }
            | Event::Interrupt { .. }
            | Event::Close { .. } => return None,
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

impl Reply {
    pub fn line(&self) -> String {
        serde_json::to_string(self).expect("답은 늘 직렬화된다")
    }
}

impl HookEvent for Request {
    fn signals(&self) -> Signals {
        let event = &self.event;
        Signals {
            step: event.name().to_owned(),
            take: event.take(),
            session: event.transcript().map(Session),
            prompt: event.prompt().map(str::to_owned),
            human_input: event.prompt().is_some(),
            turn_end: event.turn_ends(),
            reports: event.model().map(Report::Model).into_iter().collect(),
            ..Signals::default()
        }
    }

    fn show(&self, handed: &Handed) -> Shown {
        let reply = match handed {
            Handed::Taken { messages, note } => reply(self.id, messages, note.as_deref()),
            Handed::Mail(messages) => reply(self.id, messages, None),
            Handed::Nudge(text) => nudge(self.id, text.clone()),
            Handed::Interrupted => reply(self.id, &[], None),
        };
        Shown {
            out: Some(reply.line()),
            ..Shown::default()
        }
    }
}

fn reply(id: Option<u64>, taken: &[Delivery], note: Option<&str>) -> Reply {
    Reply {
        id,
        text: super::hook_text(taken, note),
        ids: taken.iter().map(|d| d.id).collect(),
        error: None,
        interrupted: false,
    }
}

fn nudge(id: Option<u64>, text: String) -> Reply {
    Reply {
        id,
        text: Some(text),
        ids: Vec::new(),
        error: None,
        interrupted: false,
    }
}

pub fn acked(id: u64) -> Reply {
    Reply {
        id: Some(id),
        text: None,
        ids: Vec::new(),
        error: None,
        interrupted: false,
    }
}

pub fn failed(id: Option<u64>, error: String) -> Reply {
    Reply {
        id,
        text: None,
        ids: Vec::new(),
        error: Some(error),
        interrupted: false,
    }
}

pub fn interrupted(id: u64) -> Reply {
    Reply {
        interrupted: true,
        ..acked(id)
    }
}
