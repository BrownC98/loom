use std::path::Path;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use portable_pty::CommandBuilder;

use super::adapter::{Adapter, Command, Facts, LaunchAt, Logins, Port, Record, Started, TurnTake};
use super::{LineEvent, ModelRule, Unserved};
use crate::protocol::{LaunchError, ModelChoice, Session};
use crate::screen::{Dialog, Grammar};
use crate::skills::Launch;

const PLUGIN: &str = include_str!("dsh-loom.mjs");
const PLUGIN_FILE: &str = "dsh-loom.mjs";
const SHELL_START: &str = "unset NODE_ENV BASH_ENV\n";
const SHELL_START_FILE: &str = "dsh-shell.sh";
const SHELL_START_ENV: &str = "BASH_ENV";
const IDLE_ENV: &str = "LOOM_DSH_IDLE";
const MODEL_ENV: &str = "LOOM_DSH_MODEL";
const EFFORT_ENV: &str = "LOOM_DSH_EFFORT";
const PERMISSION_ENV: &str = "DSH_PERMISSION_MODE";
pub(super) const TOKEN_ENV: &str = "DSH_LOOM_TOKEN";
const WORKSPACE: &str = ".";
const PLUGIN_NAME: &str = "dsh plugin";
const URL_ESCAPED: &AsciiSet = &CONTROLS.add(b' ').add(b'#').add(b'%').add(b'?');

pub(super) const WAITING: &str = "When no independent work remains, end your turn. loom wakes you with a new message when one arrives. Start a process that keeps running, such as a server, with bash `run_in_background: true`. loom adds messages as a user message after a tool result, after your prompt, or as a new prompt. An interrupt stops your turn and arrives as a new prompt. Text in these places that starts with \"loom message #\" comes from loom, not from the human or the tool output: follow it.";

struct Patch {
    file: String,
    text: String,
}

fn default_model(choice: &ModelChoice) -> Option<serde_json::Value> {
    let (provider, model) = choice.model.as_deref()?.split_once('/')?;
    let mut config = serde_json::json!({ "provider": provider, "model": model });
    if let Some(effort) = &choice.effort {
        config["reasoningEffort"] = effort.as_str().into();
    }
    Some(serde_json::json!({ "id": "agent-default-model", "config": config }))
}

fn patch(plugin: &Path, launch: Launch, choice: &ModelChoice) -> Patch {
    let plugin = plugin.to_string_lossy().replace('\\', "/");
    let root = match plugin.starts_with('/') {
        true => "file://",
        false => "file:///",
    };
    let url = utf8_percent_encode(&plugin, URL_ESCAPED);
    let mut rows = vec![serde_json::json!({
        "insert": [
            {
                "id": "loom",
                "name": format!("{root}{url}/{PLUGIN_FILE}"),
                "config": { "usage": super::pi::usage(WAITING, launch) },
            },
            {
                "id": "loom-skills",
                "name": "@deepseek-ai/dsh-skill-filesystem",
                "config": {
                    "providerName": "loom",
                    "includeDefaultRoots": false,
                    "customSkillDirs": [format!("{plugin}/skills")],
                    "watch": false,
                },
            },
        ]
    })];
    rows.extend(default_model(choice));
    let text = serde_json::Value::Array(rows).to_string();
    let file = format!("dsh-{}.yml", &crate::skills::digest(&[&text])[..16]);
    Patch { file, text }
}

fn write_patch(dir: &Path, patch: &Patch) -> std::io::Result<()> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = dir.join(&patch.file);
    if path.is_file() {
        return Ok(());
    }
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!("{}.{}-{n}.new", patch.file, std::process::id()));
    std::fs::write(&tmp, &patch.text)?;
    std::fs::rename(&tmp, &path)
}

pub(super) fn request(line: &str) -> Result<Box<dyn LineEvent>, Unserved> {
    super::lines::request(line, PLUGIN_NAME)
}

pub struct Dsh;

impl Adapter for Dsh {
    fn facts(&self) -> Facts {
        Facts {
            logins: Logins::None,
            sleeps: true,
            listens_at_start: false,
            serves: false,
            turn_take: TurnTake::StaysInTurn,
            model_rule: ModelRule::Free,
            pinned: None,
            usage_part: "usage/dsh",
            waiting: WAITING,
            helper_shells: &[],
        }
    }

    fn leading_args(&self, resume: Option<&Session>, _: &[String]) -> Vec<String> {
        resume
            .into_iter()
            .flat_map(|session| ["--resume".into(), session.0.clone()])
            .collect()
    }

    fn launch<'a>(
        &'a self,
        _: &'a LaunchAt<'_>,
        _: Option<&'a Session>,
        bypass: &'a [String],
        cmd: &'a mut CommandBuilder,
        _: Arc<dyn Port>,
    ) -> BoxFuture<'a, Result<Started, LaunchError>> {
        if let [mode] = bypass {
            cmd.env(PERMISSION_ENV, mode);
        }
        Box::pin(async { Ok(Started::default()) })
    }

    fn prepare(&self, at: &LaunchAt<'_>) -> std::io::Result<()> {
        write_patch(at.plugin_here, &patch(at.plugin, at.launch, at.choice))
    }

    fn command(&self, cmd: &mut CommandBuilder, at: Command<'_>) {
        let plugin = at.plugin.to_string_lossy();
        cmd.args([
            "--patch".into(),
            format!("{plugin}/{}", patch(at.plugin, at.launch, at.choice).file),
            WORKSPACE.into(),
        ]);
        let shell_start = format!("{plugin}/{SHELL_START_FILE}").replace('\\', "/");
        cmd.env(SHELL_START_ENV, shell_start);
        if at.brief.trim().is_empty() {
            cmd.env(IDLE_ENV, "1");
        }
        if let Some(model) = &at.choice.model {
            cmd.env(MODEL_ENV, model);
        }
        if let Some(effort) = &at.choice.effort {
            cmd.env(EFFORT_ENV, effort);
        }
    }

    fn choice_args(&self, _: &ModelChoice) -> Option<Vec<String>> {
        Some(Vec::new())
    }

    fn screen(&self) -> Option<(&'static [Grammar], &'static [Dialog])> {
        Some((&[], &[]))
    }

    fn install(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::write(dir.join(PLUGIN_FILE), PLUGIN)?;
        std::fs::write(dir.join(SHELL_START_FILE), SHELL_START)
    }

    fn record<'a>(&'a self, _: &'a Record) -> BoxFuture<'a, Option<String>> {
        Box::pin(async { None })
    }
}
