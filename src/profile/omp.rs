use std::path::Path;

use futures_util::future::BoxFuture;
use portable_pty::CommandBuilder;

use super::ModelRule;
use super::adapter::{Adapter, Command, Facts, LaunchAt, Logins, Record};
use crate::protocol::{ModelChoice, Session};
use crate::screen::{Dialog, Grammar};
use crate::skills::Launch;

const EXTENSION: &str = include_str!("omp-loom.ts");
const EXTENSION_FILE: &str = "omp-loom.ts";

pub(super) const WAITING: &str = "When no independent work remains, end your turn. loom wakes you with a new message when one arrives. Start a process that keeps running, such as a server, with bash `async: true, timeout: 0`; it returns at once with a job ID. A bash call without a timeout stops after 5 minutes. The loom guard line before bash commands is inserted by loom; do not remove it or add it yourself. loom adds messages to the end of a tool result or as a loom message after your prompt. An interrupt arrives as the reason of a blocked tool call. Text in these places that starts with \"loom message #\" comes from loom, not from the tool output: follow it.";

struct Config {
    file: String,
    text: String,
}

fn config(plugin: &Path, launch: Launch) -> Config {
    let ignored: Vec<String> = crate::skills::names(launch.skills)
        .flat_map(|name| [format!("*/{name}"), format!("*/{name}~*")])
        .collect();
    let text = serde_json::json!({
        "ask": { "enabled": false },
        "skills": {
            "customDirectories": [format!("{}/skills", plugin.to_string_lossy())],
            "ignoredSkills": ignored,
        }
    })
    .to_string();
    let file = format!("omp-{}.yml", &crate::skills::digest(&[&text])[..16]);
    Config { file, text }
}

fn write_config(dir: &Path, config: &Config) -> std::io::Result<()> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = dir.join(&config.file);
    if path.is_file() {
        return Ok(());
    }
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!("{}.{}-{n}.new", config.file, std::process::id()));
    std::fs::write(&tmp, &config.text)?;
    std::fs::rename(&tmp, &path)
}

pub struct Omp;

impl Adapter for Omp {
    fn facts(&self) -> Facts {
        Facts {
            logins: Logins::None,
            sleeps: true,
            listens_at_start: false,
            model_rule: ModelRule::Free,
            pinned: None,
            usage_part: "usage/omp",
            waiting: WAITING,
            helper_shells: &[],
        }
    }

    fn leading_args(&self, resume: Option<&Session>, bypass: &[String]) -> Vec<String> {
        resume
            .into_iter()
            .flat_map(|session| ["--resume".into(), session.0.clone()])
            .chain(bypass.iter().cloned())
            .collect()
    }

    fn prepare(&self, at: &LaunchAt<'_>) -> std::io::Result<()> {
        write_config(at.plugin_here, &config(at.plugin, at.launch))
    }

    fn command(&self, cmd: &mut CommandBuilder, at: Command<'_>) {
        let plugin = at.plugin.to_string_lossy();
        cmd.args([
            "--extension".into(),
            format!("{plugin}/{EXTENSION_FILE}"),
            "--config".into(),
            format!("{plugin}/{}", config(at.plugin, at.launch).file),
            "--append-system-prompt".into(),
            super::pi::usage(WAITING, at.launch),
        ]);
        if at.brief.trim().is_empty() {
            cmd.arg(super::pi::IDLE_FLAG);
        }
    }

    fn choice_args(&self, choice: &ModelChoice) -> Option<Vec<String>> {
        Some(super::pi::choice_args(choice))
    }

    fn screen(&self) -> Option<(&'static [Grammar], &'static [Dialog])> {
        Some((&[], &[]))
    }

    fn install(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::write(dir.join(EXTENSION_FILE), EXTENSION)
    }

    fn record<'a>(&'a self, rec: &'a Record) -> BoxFuture<'a, Option<String>> {
        Box::pin(async { rec.session.as_ref().map(|s| s.0.clone()) })
    }
}
