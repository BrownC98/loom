mod server;

use std::sync::Arc;

use futures_util::future::BoxFuture;
use portable_pty::CommandBuilder;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::adapter::{Adapter, Command, Facts, LaunchAt, Logins, Port, Record, Started, TurnTake};
use super::process::Served;
use super::{Handed, HookEvent, ModelRule, Shown, Signals, Take};
use crate::protocol::{Kind, LaunchError, ModelChoice, Session};
use crate::screen::{Dialog, Grammar};

const START_ENV: &str = "LOOM_CRUSH_START";
const HOOK: &str = "loom hook crush";
const WAITING: &str = "When no independent work remains, end your turn. loom wakes you on a new message. loom adds messages as hook context between tool calls. An interrupt cancels your current turn and starts a new prompt. Follow text that starts with \"loom message #\". Run persistent processes in the background.";

pub struct Crush;

#[derive(Serialize, Deserialize)]
struct Start {
    brief: String,
    rules: String,
    model: Option<String>,
    effort: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Conversation {
    id: String,
    data: String,
}

impl Adapter for Crush {
    fn facts(&self) -> Facts {
        Facts {
            logins: Logins::None,
            sleeps: false,
            listens_at_start: true,
            turn_take: TurnTake::CanOpenTurn,
            model_rule: ModelRule::Free,
            pinned: None,
            usage_part: "usage/crush",
            waiting: WAITING,
            helper_shells: &[HOOK],
        }
    }

    fn leading_args(&self, _: Option<&Session>, bypass: &[String]) -> Vec<String> {
        bypass.to_vec()
    }

    fn command(&self, cmd: &mut CommandBuilder, at: Command<'_>) {
        let start = Start {
            brief: at.brief.to_owned(),
            rules: format!(
                "{}\n{WAITING}\n\n{}",
                super::COMMUNICATION_USAGE,
                crate::skills::always_on(at.launch)
            ),
            model: at.choice.model.clone(),
            effort: at.choice.effort.clone(),
        };
        cmd.env(
            START_ENV,
            serde_json::to_string(&start).expect("시작 값은 직렬화된다"),
        );
    }

    fn launch<'a>(
        &'a self,
        at: &'a LaunchAt<'_>,
        resume: Option<&'a Session>,
        bypass: &'a [String],
        cmd: &'a mut CommandBuilder,
        port: Arc<dyn Port>,
    ) -> BoxFuture<'a, Result<Started, LaunchError>> {
        Box::pin(async move {
            let failed = |cause| LaunchError::Helper {
                program: "crush server".into(),
                cause,
            };
            let start: Start =
                serde_json::from_str(&cmd.get_env(START_ENV).unwrap_or_default().to_string_lossy())
                    .map_err(|e| failed(e.to_string()))?;
            cmd.env_remove(START_ENV);
            cmd.env("CRUSH_CLIENT_SERVER", "1");
            let conversation = resume
                .map(|s| serde_json::from_str::<Conversation>(&s.0))
                .transpose()
                .map_err(|e| failed(e.to_string()))?;
            let program: Vec<String> = cmd.get_argv()[..cmd.get_argv().len() - bypass.len()]
                .iter()
                .map(|s| s.to_string_lossy().into_owned())
                .collect();
            let (server, mut started) =
                server::Server::start(at, &program, conversation, &start, bypass)
                    .await
                    .map_err(failed)?;
            let conversation = server.conversation();
            cmd.args([
                "--host",
                &server.host(),
                "--data-dir",
                &conversation.data,
                "--session",
                &conversation.id,
            ]);
            started.record = Some(server.record());
            started.session = Some(Session(
                serde_json::to_string(conversation).expect("세션은 직렬화된다"),
            ));
            let brief = start.brief;
            tokio::spawn(server::drive(server, port, brief));
            Ok(started)
        })
    }

    fn attach<'a>(
        &'a self,
        rec: &'a Record,
        port: Arc<dyn Port>,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if let Some(server) = server::Server::stored(rec) {
                tokio::spawn(server::drive(server, port, String::new()));
            }
            Ok(())
        })
    }

    fn choice_args(&self, _: &ModelChoice) -> Option<Vec<String>> {
        Some(Vec::new())
    }

    fn screen(&self) -> Option<(&'static [Grammar], &'static [Dialog])> {
        None
    }

    fn end_detached<'a>(
        &'a self,
        rec: &'a Record,
        _: &'a crate::accounts::Site,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(Served::end_stored(rec))
    }

    fn shell_root(&self, rec: &Record) -> Option<u32> {
        Served::stored::<Value>(rec)?.0.here()
    }

    fn record<'a>(&'a self, _: &'a Record) -> BoxFuture<'a, Option<String>> {
        Box::pin(async { None })
    }
}

#[derive(Deserialize)]
#[serde(tag = "hook_event_name")]
pub enum Hook {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
}

impl HookEvent for Hook {
    fn signals(&self) -> Signals {
        let (name, kinds): (&str, &'static [Kind]) = match self {
            Self::PreToolUse => ("PreToolUse", &[Kind::Interrupt]),
            Self::PostToolUse => ("PostToolUse", Kind::ALL),
            Self::PostToolUseFailure => ("PostToolUseFailure", Kind::ALL),
        };
        Signals {
            step: name.into(),
            take: Some(Take::Now(kinds)),
            ..Signals::default()
        }
    }

    fn show(&self, handed: &Handed) -> Shown {
        let note = match handed {
            Handed::Taken { messages, note } => super::hook_text(messages, note.as_deref()),
            Handed::Mail(messages) => super::hook_text(messages, None),
            Handed::Nudge(text) => Some(text.clone()),
            Handed::Interrupted => Some("Stop the current task now.".into()),
        };
        let Some(text) = note else {
            return Shown::default();
        };
        let event = self.signals().step;
        let output = match self {
            Self::PreToolUse => json!({"hookSpecificOutput": {
                "hookEventName": event,
                "permissionDecision": "deny",
                "permissionDecisionReason": text,
            }}),
            _ => json!({"hookSpecificOutput": {"hookEventName": event, "additionalContext": text}}),
        };
        Shown {
            out: Some(output.to_string()),
            ..Shown::default()
        }
    }
}
