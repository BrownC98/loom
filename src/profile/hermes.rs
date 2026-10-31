use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use portable_pty::CommandBuilder;

use super::adapter::{Adapter, Command, Facts, LaunchAt, Logins, Port, Record, Started, TurnTake};
use super::{LineEvent, ModelRule, Unserved};
use crate::protocol::{LaunchError, ModelChoice, Session};
use crate::screen::{Dialog, Grammar};
use crate::wsl::Run;

const PLUGIN: &str = include_str!("hermes-loom.py");
const PLUGIN_DIR: &str = "hermes-loom";
const MANIFEST: &str =
    "name: loom\nversion: \"1\"\nkind: backend\ndescription: loom node messaging\n";
const BUNDLED: &str = include_str!("hermes-bundled.py");
const BUNDLED_FILE: &str = "hermes-bundled.py";
const OVERLAYS: &str = "hermes-bundled";
const COMMAND: &str = "hermes";
const RUNTIME_ARG: &str = "--print-runtime-command";
const MAIN_ENTRY: &str = "runpy.run_module('hermes_cli.main', run_name='__main__', alter_sys=True)";
const RUN_BUNDLED: &str =
    "exec(compile(open(sys.argv[1], encoding='utf-8').read(), sys.argv[1], 'exec'))";
const PROBE_LIMIT: Duration = Duration::from_secs(60);
const BUNDLED_ENV: &str = "HERMES_BUNDLED_PLUGINS";
const PROMPT_ENV: &str = "HERMES_EPHEMERAL_SYSTEM_PROMPT";
const IDLE_ENV: &str = "LOOM_HERMES_IDLE";
const EFFORT_ENV: &str = "LOOM_HERMES_EFFORT";
const CHAT: [&str; 2] = ["chat", "--cli"];
const PLUGIN_NAME: &str = "hermes plugin";

pub(super) const WAITING: &str = "When no independent work remains, end your turn. loom wakes you with a new message when one arrives. Start a process that keeps running, such as a server, with the terminal tool's `background: true`. loom adds messages to the end of a tool result, to your prompt, or as a new prompt. An interrupt stops your turn and arrives as a new prompt. Text in these places that starts with \"loom message #\" comes from loom, not from the human or the tool output: follow it. The loom skills are named loom:<name>; load one with skill_view.";

pub(super) fn request(line: &str) -> Result<Box<dyn LineEvent>, Unserved> {
    super::lines::request(line, PLUGIN_NAME)
}

pub struct Hermes;

impl Adapter for Hermes {
    fn facts(&self) -> Facts {
        Facts {
            logins: Logins::None,
            sleeps: true,
            listens_at_start: false,
            serves: false,
            turn_take: TurnTake::StaysInTurn,
            model_rule: ModelRule::Free,
            pinned: None,
            usage_part: "usage/hermes",
            waiting: WAITING,
            helper_shells: &[],
        }
    }

    fn leading_args(&self, resume: Option<&Session>, bypass: &[String]) -> Vec<String> {
        CHAT.into_iter()
            .map(str::to_owned)
            .chain(
                resume
                    .into_iter()
                    .flat_map(|session| ["--resume".into(), session.0.clone()]),
            )
            .chain(bypass.iter().cloned())
            .collect()
    }

    fn command(&self, cmd: &mut CommandBuilder, at: Command<'_>) {
        cmd.env(PROMPT_ENV, super::pi::usage(WAITING, at.launch));
        if at.brief.trim().is_empty() {
            cmd.env(IDLE_ENV, "1");
        }
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
            cmd.env(BUNDLED_ENV, overlay(at).await?);
            Ok(Started::default())
        })
    }

    fn choice_args(&self, choice: &ModelChoice) -> Option<Vec<String>> {
        let model = choice.model.iter().flat_map(|m| ["-m".into(), m.clone()]);
        let effort = choice
            .effort
            .iter()
            .flat_map(|e| ["--reasoning".into(), e.clone()]);
        Some(model.chain(effort).collect())
    }

    fn screen(&self) -> Option<(&'static [Grammar], &'static [Dialog])> {
        Some((&[], &[]))
    }

    fn install(&self, dir: &Path) -> std::io::Result<()> {
        let plugin = dir.join(PLUGIN_DIR);
        std::fs::create_dir_all(&plugin)?;
        std::fs::write(plugin.join("plugin.yaml"), MANIFEST)?;
        std::fs::write(plugin.join("__init__.py"), PLUGIN)?;
        std::fs::write(dir.join(BUNDLED_FILE), BUNDLED)
    }

    fn record<'a>(&'a self, _: &'a Record) -> BoxFuture<'a, Option<String>> {
        Box::pin(async { None })
    }
}

async fn overlay(at: &LaunchAt<'_>) -> Result<String, LaunchError> {
    let hermes = at
        .site
        .run_installed(COMMAND, &[RUNTIME_ARG])
        .ok_or_else(|| LaunchError::NotInstalled {
            agent: "Hermes".into(),
            command: COMMAND.into(),
            distro: at.site.distro().map(str::to_owned),
        })?;
    let probe = format!("{COMMAND} {RUNTIME_ARG}");
    let runtime: Vec<String> = serde_json::from_slice(&output(hermes, &probe).await?)
        .map_err(|e| helper(&probe, format!("응답이 JSON 목록이 아니다: {e}")))?;
    let [python, isolated, flag, code, ..] = runtime.as_slice() else {
        return Err(helper(&probe, "응답의 항목이 4개보다 적다".into()));
    };
    let start = code
        .strip_suffix(MAIN_ENTRY)
        .filter(|_| isolated == "-I" && flag == "-c")
        .ok_or_else(|| helper(&probe, format!("명령이 {MAIN_ENTRY}로 끝나지 않는다")))?;
    let plugin = at.plugin.to_string_lossy().replace('\\', "/");
    let overlays = match at.inside {
        Some(inside) => format!("{}/{OVERLAYS}", inside.run_dir()),
        None => at.run_dir.join(OVERLAYS).to_string_lossy().into_owned(),
    };
    let code = format!("{start}{RUN_BUNDLED}");
    let script = format!("{plugin}/{BUNDLED_FILE}");
    let loom = format!("{plugin}/{PLUGIN_DIR}");
    let args = ["-I", "-c", &code, &script, &loom, &overlays];
    let out = output(at.site.run_program(python, &args), BUNDLED_FILE).await?;
    String::from_utf8_lossy(&out)
        .lines()
        .last()
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| helper(BUNDLED_FILE, "응답에 폴더가 없다".into()))
}

async fn output(run: Run, program: &str) -> Result<Vec<u8>, LaunchError> {
    let ran = tokio::task::spawn_blocking(move || run.output(PROBE_LIMIT))
        .await
        .map_err(|e| helper(program, e.to_string()))?
        .map_err(|e| helper(program, e))?;
    match ran.code {
        Some(0) => Ok(ran.stdout.into_bytes()),
        code => Err(helper(
            program,
            format!(
                "{}: {}",
                code.map_or("no exit code".to_owned(), |c| format!("exit code {c}")),
                ran.stderr.trim()
            ),
        )),
    }
}

fn helper(program: &str, cause: String) -> LaunchError {
    LaunchError::Helper {
        program: program.into(),
        cause,
    }
}
