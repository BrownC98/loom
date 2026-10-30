use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use portable_pty::CommandBuilder;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::adapter::{Adapter, Command, Facts, LaunchAt, Logins, Port, Record, Started, TurnTake};
use super::claude::Event;
use super::process::Served;
use super::{Handed, HookEvent, ModelRule, Shown, Signals, Take};
use crate::protocol::{LaunchError, Model, ModelChoice, Report, Session};
use crate::screen::{Dialog, Grammar};

const HOOK: &str = "loom hook qwen";
const HOOK_TIMEOUT_SECS: u64 = 10;
const INPUT_ENV: &str = "LOOM_QWEN_INPUT";
const WAKE_PROMPT: &str =
    "loom has a pending message. Follow the loom hook context, then end your turn.";
const WAITING: &str = "When no independent work remains, end your turn. loom wakes you when a message arrives. The loom guard line before foreground shell commands is inserted by loom. Do not remove it or add it yourself. loom adds messages as hook additional context after a tool call or at a prompt. An interrupt arrives as the reason of a denied tool call. A wake arrives at a new prompt. Text in these places that starts with \"loom message #\" comes from loom: follow it.";

pub struct Qwen;

impl Adapter for Qwen {
    fn facts(&self) -> Facts {
        Facts {
            logins: Logins::None,
            sleeps: true,
            listens_at_start: true,
            turn_take: TurnTake::CanOpenTurn,
            model_rule: ModelRule::Free,
            pinned: None,
            usage_part: "usage/qwen-code",
            waiting: WAITING,
            helper_shells: &[HOOK],
        }
    }

    fn leading_args(&self, resume: Option<&Session>, bypass: &[String]) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(session) = resume {
            args.extend(["--resume".into(), super::session_id(session)]);
        }
        args.extend_from_slice(bypass);
        args
    }

    fn command(&self, cmd: &mut CommandBuilder, at: Command<'_>) {
        cmd.args([
            "--append-system-prompt".to_owned(),
            format!(
                "{}\n{WAITING}\n\n{}",
                super::COMMUNICATION_USAGE,
                crate::skills::always_on(at.launch),
            ),
        ]);
        cmd.env(
            "QWEN_CODE_SYSTEM_SETTINGS_PATH",
            at.plugin.join(config(at.choice).file),
        );
    }

    fn launch<'a>(
        &'a self,
        at: &'a LaunchAt<'_>,
        _: Option<&'a Session>,
        _: &'a [String],
        cmd: &'a mut CommandBuilder,
        port: Arc<dyn Port>,
    ) -> BoxFuture<'a, Result<Started, LaunchError>> {
        Box::pin(async move {
            let failed = |cause: String| LaunchError::Helper {
                program: "qwen --input-file".into(),
                cause,
            };
            let name = format!(
                "qwen-{:016x}.jsonl",
                getrandom::u64().map_err(|e| failed(e.to_string()))?
            );
            let (seen, file) = match at.inside {
                Some(inside) => {
                    let seen = format!("{}/{name}", inside.run_dir());
                    let file = inside.local(&seen);
                    (seen, file)
                }
                None => {
                    let file = at.run_dir.join(name);
                    (file.display().to_string(), file)
                }
            };
            std::fs::write(&file, "").map_err(|e| failed(e.to_string()))?;
            cmd.args(["--input-file", &seen]);
            cmd.env(INPUT_ENV, &seen);
            let record = Served {
                pid: None,
                distro: None,
            }
            .record(Input { file: file.clone() });
            tokio::spawn(wake(file, port));
            Ok(Started {
                record: Some(record),
                ..Started::default()
            })
        })
    }

    fn attach<'a>(
        &'a self,
        rec: &'a Record,
        port: Arc<dyn Port>,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if let Some((_, input)) = Served::stored::<Input>(rec) {
                tokio::spawn(wake(input.file, port));
            }
            Ok(())
        })
    }

    fn launch_settings(&self) -> Option<String> {
        Some(settings().to_string())
    }

    fn prepare(&self, at: &LaunchAt<'_>) -> std::io::Result<()> {
        let config = config(at.choice);
        let file = at.plugin_here.join(&config.file);
        if file.is_file() {
            return Ok(());
        }
        let draft = at.plugin_here.join(format!(
            "{}-{}.new",
            config.file,
            getrandom::u64().map_err(std::io::Error::other)?
        ));
        let written =
            std::fs::write(&draft, config.text).and_then(|()| std::fs::rename(&draft, &file));
        if let Err(e) = written {
            let _ = std::fs::remove_file(draft);
            if !file.is_file() {
                return Err(e);
            }
        }
        Ok(())
    }

    fn choice_args(&self, choice: &ModelChoice) -> Option<Vec<String>> {
        let mut args = Vec::new();
        if let Some(model) = &choice.model {
            args.extend(["--model".into(), model.clone()]);
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

#[derive(Serialize, Deserialize)]
struct Input {
    file: PathBuf,
}

async fn wake(file: PathBuf, port: Arc<dyn Port>) {
    let pending = file.with_extension("pending");
    let mut on = || {
        let created = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&pending);
        match created {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return,
            Err(e) => {
                eprintln!("loom: Qwen 깨움 표지를 쓰지 못했다: {e}");
                return;
            }
            Ok(_) => {}
        }
        let row = json!({"type": "submit", "text": WAKE_PROMPT});
        let written = std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .and_then(|mut out| writeln!(out, "{row}"));
        if let Err(e) = written {
            let _ = std::fs::remove_file(&pending);
            eprintln!("loom: Qwen 깨움을 쓰지 못했다: {e}");
        }
    };
    tokio::select! {
        refused = port.wakes(&mut on) => eprintln!("loom: Qwen 깨움을 멈췄다: {refused}"),
        () = port.ended() => {}
    }
    for path in [pending, file] {
        if let Err(e) = std::fs::remove_file(&path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("loom: {}을 지우지 못했다: {e}", path.display());
        }
    }
}

struct Config {
    file: String,
    text: String,
}

fn config(choice: &ModelChoice) -> Config {
    let mut config = settings();
    if let Some(effort) = &choice.effort {
        config["model"] = json!({"reasoningEffort": effort});
    }
    let text = config.to_string();
    Config {
        file: format!("qwen-{}.json", &crate::skills::digest(&[&text])[..16]),
        text,
    }
}

fn settings() -> Value {
    let hook = json!([{ "matcher": "*", "hooks": [{
        "type": "command", "command": HOOK, "timeout": HOOK_TIMEOUT_SECS,
    }] }]);
    json!({ "hooks": {
        "SessionStart": hook,
        "UserPromptSubmit": hook,
        "PreToolUse": hook,
        "PostToolUse": hook,
        "PostToolUseFailure": hook,
        "Stop": hook,
    } })
}

#[derive(Deserialize)]
pub struct Hook {
    #[serde(flatten)]
    event: Event,
    session_id: String,
    transcript_path: Option<String>,
    last_assistant_message: Option<String>,
    agent_id: Option<String>,
    prompt_id: Option<String>,
}

impl HookEvent for Hook {
    fn signals(&self) -> Signals {
        let main = self.agent_id.is_none();
        if main && let Some(file) = std::env::var_os(INPUT_ENV) {
            let pending = PathBuf::from(file).with_extension("pending");
            match self.event {
                Event::UserPromptSubmit { .. } => {
                    if let Err(e) = std::fs::OpenOptions::new()
                        .write(true)
                        .create(true)
                        .truncate(false)
                        .open(&pending)
                    {
                        eprintln!("loom: Qwen 턴 표지를 쓰지 못했다: {e}");
                    }
                }
                Event::Stop => {
                    if let Err(e) = std::fs::remove_file(&pending)
                        && e.kind() != std::io::ErrorKind::NotFound
                    {
                        eprintln!("loom: Qwen 턴 표지를 지우지 못했다: {e}");
                    }
                }
                _ => {}
            }
        }
        let waking = self.event.prompt() == Some(WAKE_PROMPT);
        let take = match self.event {
            Event::SessionStart { .. } => Some(Take::ListenStart),
            Event::Stop => Some(Take::ListenEnd),
            Event::UserPromptSubmit { .. } if waking => Some(Take::Woken),
            _ => self.event.take(),
        }
        .filter(|_| main);
        let model = match &self.event {
            Event::SessionStart { model } => Some(Report::Model(Model {
                model: model.clone(),
                effort: None,
            })),
            _ => None,
        };
        Signals {
            step: self.event.name().into(),
            take,
            session: main.then(|| {
                Session(
                    self.transcript_path
                        .clone()
                        .unwrap_or_else(|| self.session_id.clone()),
                )
            }),
            prompt: self.event.prompt().map(str::to_owned),
            human_input: !waking && self.event.prompt().is_some_and(|prompt| !prompt.is_empty()),
            prompt_id: self.prompt_id.clone().filter(|_| !waking),
            turn_end: (main && matches!(self.event, Event::Stop))
                .then(|| self.last_assistant_message.clone()),
            asks_interrupt: !main && matches!(self.event, Event::PreToolUse { .. }),
            reports: model.into_iter().collect(),
            ..Signals::default()
        }
    }

    fn show(&self, handed: &Handed) -> Shown {
        let text = match handed {
            Handed::Taken { messages, note } => super::hook_text(messages, note.as_deref()),
            Handed::Mail(messages) => super::hook_text(messages, None),
            Handed::Nudge(text) => Some(text.clone()),
            Handed::Interrupted => Some(
                "Your parent received a loom interrupt. Stop and return to your parent.".into(),
            ),
        };
        let out = match (&self.event, text) {
            (Event::Stop, Some(text)) => Some(json!({ "decision": "block", "reason": text })),
            (Event::SessionStart { .. }, Some(text)) => Some(json!({ "hookSpecificOutput": {
                "hookEventName": "SessionStart", "additionalContext": text,
            } })),
            (_, text) => self.event.reply(&[], text.as_deref()),
        };
        Shown {
            out: out.map(|out| out.to_string()),
            ..Shown::default()
        }
    }
}
