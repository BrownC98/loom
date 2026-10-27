use std::path::Path;

use futures_util::future::BoxFuture;
use portable_pty::CommandBuilder;

use super::adapter::{Adapter, Command, Facts, Logins, Record};
use super::{LineEvent, ModelRule, Unserved};
use crate::protocol::{ModelChoice, Session};
use crate::screen::{Dialog, Grammar};
use crate::skills::Launch;

const EXTENSION: &str = include_str!("pi-loom.ts");
const EXTENSION_FILE: &str = "pi-loom.ts";
const HOOK: &str = include_str!("loom-hook.ts");
const EXTENSIONS: [(&str, &str); 2] = [("loom-hook.ts", HOOK), (EXTENSION_FILE, EXTENSION)];
pub(super) const IDLE_FLAG: &str = "--loom-idle";
const EXTENSION_NAME: &str = "pi extension";

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
            model_rule: ModelRule::Free,
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

    fn command(&self, cmd: &mut CommandBuilder, at: Command<'_>) {
        cmd.args(args(at.plugin, at.launch, at.brief));
    }

    fn choice_args(&self, choice: &ModelChoice) -> Option<Vec<String>> {
        Some(choice_args(choice))
    }

    fn screen(&self) -> Option<(&'static [Grammar], &'static [Dialog])> {
        Some((&[], &[]))
    }

    fn install(&self, dir: &Path) -> std::io::Result<()> {
        EXTENSIONS
            .iter()
            .try_for_each(|(file, text)| std::fs::write(dir.join(file), text))
    }

    fn record<'a>(&'a self, rec: &'a Record) -> BoxFuture<'a, Option<String>> {
        Box::pin(async { rec.session.as_ref().map(|s| s.0.clone()) })
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

pub(super) fn request(line: &str) -> Result<Box<dyn LineEvent>, Unserved> {
    super::lines::request(line, EXTENSION_NAME)
}
