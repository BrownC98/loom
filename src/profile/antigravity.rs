use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use portable_pty::CommandBuilder;
use serde::Deserialize;
use serde_json::{Value, json};

use super::adapter::{Adapter, Command, Facts, LaunchAt, Logins, Port, Record, Started, TurnTake};
use super::{Handed, HookEvent, ModelRule, Report, Shown, Signals, Take};
use crate::protocol::{Kind, LaunchError, Model, ModelChoice, Session};
use crate::screen::{Dialog, Grammar};
use crate::skills::Launch;

const HOOK_TIMEOUT_SECS: u64 = 10;
const IDLE_HOOK_TIMEOUT_SECS: u64 = 2_000_000;
const FOLDER: &str = "antigravity";
const EFFORT_ENV: &str = "LOOM_AGY_EFFORT";
const CUSTOMIZATIONS: &str = ".agents";
const RULES_FILE: &str = "AGENTS.md";
const HOOK_NAME: &str = "loom";
const USER_INPUT: &str = "USER_INPUT";
const TRANSCRIPT_TAIL_BYTES: u64 = 1024 * 1024;
const PRE_INVOCATION: &str = "loom hook antigravity-pre-invocation";
const PRE_TOOL_USE: &str = "loom hook antigravity-pre-tool-use";
const STOP: &str = "loom hook antigravity-stop";
const HELPER_SHELLS: [&str; 3] = [PRE_INVOCATION, PRE_TOOL_USE, STOP];

pub(super) const WAITING: &str = "When no independent work remains, end your turn. loom wakes you with a new message when one arrives. loom adds messages as a system message before your next step. An interrupt arrives as the reason of a denied tool call. Text in these places that starts with \"loom message #\" comes from loom, not from the tool output: follow it.";

fn rules(launch: Launch) -> String {
    format!(
        "{}\n{WAITING}\n\n{}\n",
        super::COMMUNICATION_USAGE,
        crate::skills::always_on(launch)
    )
}

fn hooks() -> Value {
    let handler = |command: &str, timeout: u64| json!([{ "type": "command", "command": command, "timeout": timeout }]);
    json!({
        HOOK_NAME: {
            "PreInvocation": handler(PRE_INVOCATION, HOOK_TIMEOUT_SECS),
            "PreToolUse": [{
                "matcher": "*",
                "hooks": handler(PRE_TOOL_USE, HOOK_TIMEOUT_SECS),
            }],
            "Stop": handler(STOP, IDLE_HOOK_TIMEOUT_SECS),
        },
    })
}

fn write_workspace(dir: &Path, rules: &str, hooks: &str, skills: &str) -> std::io::Result<()> {
    let customizations = dir.join(CUSTOMIZATIONS);
    std::fs::create_dir_all(&customizations)?;
    replace(&dir.join(RULES_FILE), rules)?;
    replace(&customizations.join("hooks.json"), hooks)?;
    replace(&customizations.join("skills.json"), skills)
}

fn replace(path: &Path, text: &str) -> std::io::Result<()> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let draft = path.with_extension(format!("{}-{n}.new", std::process::id()));
    std::fs::write(&draft, text)?;
    std::fs::rename(&draft, path)
}

pub struct Antigravity;

impl Adapter for Antigravity {
    fn facts(&self) -> Facts {
        Facts {
            logins: Logins::None,
            sleeps: true,
            listens_at_start: false,
            turn_take: TurnTake::StaysInTurn,
            model_rule: ModelRule::Free,
            pinned: None,
            usage_part: "usage/antigravity",
            waiting: WAITING,
            helper_shells: &HELPER_SHELLS,
        }
    }

    fn idle_holds_input(&self) -> bool {
        true
    }

    fn leading_args(&self, resume: Option<&Session>, bypass: &[String]) -> Vec<String> {
        resume
            .into_iter()
            .flat_map(|session| ["--conversation".into(), session.0.clone()])
            .chain(bypass.iter().cloned())
            .collect()
    }

    fn command(&self, cmd: &mut CommandBuilder, at: Command<'_>) {
        if let Some(effort) = &at.choice.effort {
            cmd.env(EFFORT_ENV, effort);
        }
    }

    fn launch<'a>(
        &'a self,
        at: &'a LaunchAt<'_>,
        _: Option<&'a Session>,
        _: &'a [String],
        cmd: &'a mut CommandBuilder,
        _: Arc<dyn Port>,
    ) -> BoxFuture<'a, Result<Started, LaunchError>> {
        Box::pin(async move {
            let rules = rules(at.launch);
            let hooks = hooks().to_string();
            let skills = json!({ "entries": [{ "path": at.skills }] }).to_string();
            let hash = crate::skills::digest(&[&rules, &hooks, &skills]);
            let name = format!("{FOLDER}/{hash}");
            let (seen, here) = match at.inside {
                Some(inside) => {
                    let seen = format!("{}/{name}", inside.run_dir());
                    let here = inside.local(&seen);
                    (seen, here)
                }
                None => {
                    let here = at.run_dir.join(name);
                    (here.to_string_lossy().into_owned(), here)
                }
            };
            write_workspace(&here, &rules, &hooks, &skills).map_err(|error| {
                LaunchError::Helper {
                    program: FOLDER.into(),
                    cause: error.to_string(),
                }
            })?;
            cmd.args(["--add-dir", &seen]);
            Ok(Started::default())
        })
    }

    fn choice_args(&self, choice: &ModelChoice) -> Option<Vec<String>> {
        let mut args = Vec::new();
        if let Some(model) = &choice.model {
            args.extend(["--model".into(), model.clone()]);
        }
        if let Some(effort) = &choice.effort {
            args.extend(["--effort".into(), effort.clone()]);
        }
        Some(args)
    }

    fn screen(&self) -> Option<(&'static [Grammar], &'static [Dialog])> {
        None
    }

    fn record<'a>(&'a self, rec: &'a Record) -> BoxFuture<'a, Option<String>> {
        Box::pin(async { rec.session.as_ref().map(|s| s.0.clone()) })
    }
}

#[derive(Clone, Copy)]
enum Event {
    PreInvocation,
    PreToolUse,
    Stop,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Input {
    conversation_id: Option<String>,
    model_name: Option<String>,
    last_user_input: Option<String>,
    transcript_path: Option<String>,
    #[serde(default)]
    invocation_num: u64,
    #[serde(default)]
    fully_idle: bool,
}

struct Hook {
    event: Event,
    input: Input,
    prompted: bool,
}

pub fn pre_invocation(input: Value) -> Result<Box<dyn HookEvent>, serde_json::Error> {
    Ok(Box::new(Hook::parse(Event::PreInvocation, input)?))
}

pub fn pre_tool_use(input: Value) -> Result<Box<dyn HookEvent>, serde_json::Error> {
    Ok(Box::new(Hook::parse(Event::PreToolUse, input)?))
}

pub fn stop(input: Value) -> Result<Box<dyn HookEvent>, serde_json::Error> {
    Ok(Box::new(Hook::parse(Event::Stop, input)?))
}

impl Hook {
    fn parse(event: Event, input: Value) -> Result<Self, serde_json::Error> {
        let input = Input::deserialize(input)?;
        let prompted = matches!(event, Event::PreInvocation)
            && input.invocation_num == 0
            && input
                .transcript_path
                .as_deref()
                .is_some_and(last_step_typed);
        Ok(Self {
            event,
            input,
            prompted,
        })
    }

    fn idle(&self) -> bool {
        matches!(self.event, Event::Stop) && self.input.fully_idle
    }

    fn model(&self) -> Report {
        Report::Model(Model {
            model: self.input.model_name.clone(),
            effort: std::env::var(EFFORT_ENV).ok(),
        })
    }

    fn step(&self) -> &'static str {
        match self.event {
            Event::PreInvocation => "PreInvocation",
            Event::PreToolUse => "PreToolUse",
            Event::Stop => "Stop",
        }
    }
}

impl HookEvent for Hook {
    fn signals(&self) -> Signals {
        Signals {
            step: self.step().into(),
            take: Some(match self.event {
                Event::PreToolUse => Take::Now(&[Kind::Interrupt]),
                Event::Stop if self.idle() => Take::WhenIdle,
                Event::PreInvocation | Event::Stop => Take::Now(Kind::ALL),
            }),
            session: self.input.conversation_id.clone().map(Session),
            human_input: self.prompted,
            prompt: self.input.last_user_input.clone().filter(|_| self.prompted),
            turn_end: self.idle().then_some(None),
            reports: (self.prompted || self.idle())
                .then(|| self.model())
                .into_iter()
                .collect(),
            ..Signals::default()
        }
    }

    fn show(&self, handed: &Handed) -> Shown {
        let text = match handed {
            Handed::Taken { messages, note } => super::hook_text(messages, note.as_deref()),
            Handed::Mail(messages) => super::hook_text(messages, None),
            Handed::Nudge(text) => Some(text.clone()),
            Handed::Interrupted => None,
        };
        let out = match (self.event, text) {
            (Event::PreInvocation, Some(text)) => {
                json!({ "injectSteps": [{ "systemMessage": { "systemMessage": text } }] })
            }
            (Event::PreToolUse, Some(text)) => json!({ "decision": "deny", "reason": text }),
            (Event::PreToolUse, None) => json!({ "decision": "ask" }),
            (Event::Stop, Some(text)) => json!({ "decision": "continue", "reason": text }),
            (Event::PreInvocation | Event::Stop, None) => json!({}),
        };
        Shown {
            out: Some(out.to_string()),
            ..Shown::default()
        }
    }
}

#[derive(Deserialize)]
struct Step {
    r#type: String,
}

fn last_step_typed(transcript: &str) -> bool {
    let here = std::env::current_dir().unwrap_or_default();
    let path = crate::wsl::from_node(transcript.into(), &here);
    match last_line(&path) {
        Ok(line) => serde_json::from_str::<Step>(&line).is_ok_and(|step| step.r#type == USER_INPUT),
        Err(e) => {
            eprintln!(
                "loom: could not read the transcript {}: {e}",
                path.display()
            );
            false
        }
    }
}

fn last_line(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TRANSCRIPT_TAIL_BYTES)))?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail)?;
    Ok(String::from_utf8_lossy(&tail)
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .to_owned())
}
