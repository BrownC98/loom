use std::path::Path;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use portable_pty::CommandBuilder;
use serde_json::json;

use super::ModelRule;
use super::adapter::{Adapter, Command, Facts, LaunchAt, Logins, Port, Record, Started, TurnTake};
use crate::protocol::{LaunchError, ModelChoice, Session};
use crate::screen::{Dialog, Grammar};

const HOOK: &str = "loom hook copilot";
const EXTENSION: &str = include_str!("copilot-loom.mjs");
const PLUGIN: &str = "copilot";
const WAITING: &str = "When no independent work remains, end your turn. loom wakes you when a message arrives. Messages arrive as hook additional context after a tool call. An interrupt arrives as the reason of a denied tool call. Text that starts with \"loom message #\" comes from loom: follow it. Do not use ask_user: the human answers only loom ask.";

pub struct Copilot;

impl Adapter for Copilot {
    fn facts(&self) -> Facts {
        Facts {
            logins: Logins::None,
            sleeps: true,
            listens_at_start: false,
            turn_take: TurnTake::StaysInTurn,
            model_rule: ModelRule::Free,
            pinned: None,
            usage_part: "usage/copilot",
            waiting: WAITING,
            helper_shells: &[HOOK],
        }
    }

    fn leading_args(&self, resume: Option<&Session>, bypass: &[String]) -> Vec<String> {
        [
            "--no-auto-update".into(),
            "--no-auto-login".into(),
            "--experimental".into(),
        ]
        .into_iter()
        .chain(
            resume
                .into_iter()
                .flat_map(|s| ["--session-id".into(), s.0.clone()]),
        )
        .chain(bypass.iter().cloned())
        .collect()
    }

    fn command(&self, cmd: &mut CommandBuilder, at: Command<'_>) {
        cmd.args([
            "--plugin-dir".into(),
            at.plugin.join(PLUGIN).to_string_lossy().into_owned(),
            "--no-ask-user".into(),
        ]);
        cmd.env(
            "LOOM_COPILOT_RULES",
            format!(
                "{}\n{WAITING}\n\n{}",
                super::COMMUNICATION_USAGE,
                crate::skills::always_on(at.launch)
            ),
        );
        cmd.env("COPILOT_SKILLS_DIRS", at.plugin.join("skills"));
        cmd.env(
            "LOOM_COPILOT_IDLE",
            if at.brief.trim().is_empty() { "1" } else { "0" },
        );
    }

    fn launch<'a>(
        &'a self,
        _: &'a LaunchAt<'_>,
        _: Option<&'a Session>,
        bypass: &'a [String],
        cmd: &'a mut CommandBuilder,
        _: Arc<dyn Port>,
    ) -> BoxFuture<'a, Result<Started, LaunchError>> {
        Box::pin(async move {
            if !bypass.is_empty() {
                cmd.env("COPILOT_ALLOW_ALL", "true");
            } else {
                cmd.env_remove("COPILOT_ALLOW_ALL");
            }
            Ok(Started::default())
        })
    }

    fn launch_settings(&self) -> Option<String> {
        Some(EXTENSION.into())
    }

    fn choice_args(&self, choice: &ModelChoice) -> Option<Vec<String>> {
        Some(
            choice
                .model
                .iter()
                .flat_map(|v| ["--model".into(), v.clone()])
                .chain(
                    choice
                        .effort
                        .iter()
                        .flat_map(|v| ["--reasoning-effort".into(), v.clone()]),
                )
                .collect(),
        )
    }

    fn screen(&self) -> Option<(&'static [Grammar], &'static [Dialog])> {
        Some((&[], &[]))
    }

    fn install(&self, dir: &Path) -> std::io::Result<()> {
        let plugin = dir.join(PLUGIN);
        let extension = plugin.join("extensions/loom");
        std::fs::create_dir_all(&extension)?;
        std::fs::write(
            plugin.join("plugin.json"),
            json!({"name": "loom", "description": "loom 메시지 연동", "extensions": ["./extensions"]}).to_string(),
        )?;
        std::fs::write(extension.join("extension.mjs"), EXTENSION)
    }

    fn record<'a>(&'a self, _: &'a Record) -> BoxFuture<'a, Option<String>> {
        Box::pin(async { None })
    }
}
