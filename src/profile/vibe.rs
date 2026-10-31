use std::path::Path;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use portable_pty::CommandBuilder;
use serde_json::json;

use super::adapter::{Adapter, Command, Facts, LaunchAt, Logins, Port, Record, Started, TurnTake};
use super::process::Served;
use super::{LineEvent, ModelRule, Unserved, VersionMismatch};
use crate::protocol::{LaunchError, ModelChoice, Session};
use crate::screen::{Dialog, Grammar};

const BRIDGE: &str = include_str!("vibe-loom.py");
const FILE: &str = "vibe-loom.py";
const PINNED: &str = "3.0.3";
const WAITING: &str = "When no independent work remains, end your turn. loom starts a new turn when mail arrives. loom adds messages through the session input queue. An interrupt stops your turn. Follow text that starts with \"loom message #\" from these paths.";

pub struct Vibe;

impl Adapter for Vibe {
    fn facts(&self) -> Facts {
        Facts {
            logins: Logins::None,
            sleeps: true,
            listens_at_start: false,
            serves: false,
            turn_take: TurnTake::CanOpenTurn,
            model_rule: ModelRule::Free,
            pinned: Some("vibe"),
            usage_part: "usage/vibe",
            waiting: WAITING,
            helper_shells: &["loom hook vibe-bridge"],
        }
    }

    fn leading_args(&self, resume: Option<&Session>, bypass: &[String]) -> Vec<String> {
        resume
            .into_iter()
            .flat_map(|s| ["--resume".into(), s.0.clone()])
            .chain(bypass.iter().cloned())
            .collect()
    }

    fn command(&self, cmd: &mut CommandBuilder, at: Command<'_>) {
        let cli = cmd.get_argv()[0].clone();
        let python = if cfg!(windows) { "python" } else { "python3" };
        cmd.get_argv_mut().splice(
            0..1,
            [
                python.into(),
                at.plugin.join(FILE).into_os_string(),
                "launch".into(),
                cli,
            ],
        );
        cmd.env("LOOM_VIBE_USAGE", super::pi::usage(WAITING, at.launch));
        cmd.env("LOOM_VIBE_VERSION", PINNED);
        cmd.env("LOOM_VIBE_SKILLS", at.plugin.join("skills"));
        if let Some(model) = &at.choice.model {
            cmd.env("LOOM_VIBE_MODEL", model);
        }
        if let Some(effort) = &at.choice.effort {
            cmd.env("LOOM_VIBE_EFFORT", effort);
        }
        cmd.env(
            "LOOM_VIBE_IDLE",
            if at.brief.trim().is_empty() { "1" } else { "0" },
        );
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
            if at.inside.is_some() {
                cmd.get_argv_mut()[0] = "python3".into();
            }
            Ok(Started {
                record: Some(
                    Served {
                        pid: None,
                        distro: None,
                    }
                    .record(json!({ "bridge": FILE, "version": PINNED })),
                ),
                ..Started::default()
            })
        })
    }

    fn choice_args(&self, _: &ModelChoice) -> Option<Vec<String>> {
        Some(Vec::new())
    }

    fn screen(&self) -> Option<(&'static [Grammar], &'static [Dialog])> {
        Some((&[], &[]))
    }

    fn install(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::write(dir.join(FILE), BRIDGE)
    }

    fn version_warning(&self, output: &str) -> Option<VersionMismatch> {
        let installed = output.split_whitespace().last()?.trim_start_matches('v');
        (installed != PINNED).then(|| VersionMismatch {
            installed: installed.into(),
            pinned: PINNED.into(),
        })
    }

    fn record<'a>(&'a self, _: &'a Record) -> BoxFuture<'a, Option<String>> {
        Box::pin(async { None })
    }
}

pub(super) fn request(line: &str) -> Result<Box<dyn LineEvent>, Unserved> {
    super::lines::request(line, "Vibe bridge")
}
