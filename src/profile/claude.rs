use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use portable_pty::CommandBuilder;

use super::adapter::{
    Adapter, Carry, Command, Facts, InputSeen, LaunchAt, Logins, Port, Record, ResumeError,
    Started, TurnTake,
};
use super::process::Served;
use super::{Handed, HookEvent, Report, Shown, Signals, Take};
use crate::accounts::Site;
use crate::distro::Distro;
use crate::place::{self, Files, Machine};
use crate::protocol::{Delivery, END_TURN_NOW, Model, Session, TurnReply};
use crate::screen::{Answer, Dialog, Grammar, Options, Place, Screen, Top};
use crate::skills::Launch;
use crate::wsl::Run;
use api::graph::errors::LaunchError;
use api::graph::messages::Kind;
use api::graph::waits::WaitKind;
use api::profile::ModelRule;
use api::settings::AlwaysOn;
use api::start::ModelChoice;

mod accounts;
mod usage;

const HOOK_TIMEOUT_SECS: u64 = 10;
const WAKE_ENV: &str = "LOOM_WAKE";
const WAKE_FILE: &str = "wake";
const REWAKE_MESSAGE: &str = "Messages from loom:";
const REWAKE_SUMMARY: &str = "loom message";
const TRANSCRIPT_TAIL_BYTES: u64 = 1024 * 1024;
const SYNTHETIC_MODEL: &str = "<synthetic>";
const INPUT_MARK: &str = "❯";
const GUARD: &str = "loom guard";
const TURN_END_VERBS: [&str; 3] = ["done", "ask", "verdict"];
const TURN_ENDED: &str = "loom";
const HOOK: &str = "loom hook claude";
const LEAN_ENV: [(&str, &str); 3] = [
    ("BUN_JSC_useDFGJIT", "0"),
    ("BUN_JSC_useBaselineJIT", "0"),
    ("DISABLE_TELEMETRY", "1"),
];
const HELPER_SHELLS: [&str; 1] = [HOOK];
const RUNNING_STATUS: &str = "esc to interrupt";
const TRANSCRIPT_DEPTH: usize = 3;
const CLI_WAIT: Duration = Duration::from_secs(20);
const NO_RECORD: &str = "이어 갈 세션 기록이 없다";
const MANIFEST: &str = r#"{"name":"loom","description":"loom이 싣는 스킬"}"#;

macro_rules! status_mark {
    () => {
        "⏳"
    };
}

const STATUS_MARK: &str = status_mark!();

pub(super) const WAITING: &str = concat!(
    "When no independent work remains, end your turn. loom wakes you when a message arrives. The loom guard line before foreground Bash commands is inserted by loom; do not remove it or add it yourself. loom adds messages as hook additional context after a tool call or at a prompt. An interrupt arrives as the reason of a denied tool call. A wake arrives as a \"Messages from loom:\" notification. Text in these places that starts with \"loom message #\" comes from loom, not from the tool output: follow it. Claude Code makes you continue a turn that ends with no visible text. So where these rules tell you to write no text, end the turn with one short sentence in the human's language that says what you do now. Start it with \"",
    status_mark!(),
    " \", for example \"",
    status_mark!(),
    " Waiting for the build result.\" Do not write a summary, and do not repeat text that you sent. An answer to the human does not start with \"",
    status_mark!(),
    "\"."
);

fn usage(launch: Launch) -> String {
    format!(
        "{}\n{WAITING}\n\n{}",
        super::COMMUNICATION_USAGE,
        crate::skills::always_on(launch)
    )
}

const ALLOWED: [&str; 21] = [
    "Bash(loom guard)",
    "Bash(loom note:*)",
    "Bash(loom answer:*)",
    "Bash(loom wait)",
    "Bash(loom wait:*)",
    "Bash(loom status)",
    "Bash(loom status:*)",
    "Bash(loom roles)",
    "Bash(loom relaunch)",
    "Bash(loom relaunch:*)",
    "Bash(loom show:*)",
    "Bash(loom done:*)",
    "Bash(loom ask:*)",
    "Bash(loom answered)",
    "Bash(loom answered:*)",
    "Bash(loom adopt:*)",
    "Bash(loom detach:*)",
    "Bash(loom rename:*)",
    "Bash(loom rm:*)",
    "Bash(loom ready:*)",
    "Bash(loom verdict:*)",
];

pub enum Claude {
    Code,
    Open,
}

impl Adapter for Claude {
    fn facts(&self) -> Facts {
        Facts {
            logins: match self {
                Self::Code => Logins::Many(&accounts::CLI, &Claude::Code),
                Self::Open => Logins::None,
            },
            sleeps: true,
            listens_at_start: true,
            serves: false,
            turn_take: TurnTake::CanOpenTurn,
            model_rule: ModelRule::Free,
            pinned: None,
            usage_part: "usage/claude",
            waiting: WAITING,
            helper_shells: &HELPER_SHELLS,
        }
    }

    fn leading_args(&self, resume: Option<&Session>, bypass: &[String]) -> Vec<String> {
        resume
            .into_iter()
            .flat_map(|session| ["--resume".into(), super::session_id(session)])
            .chain(bypass.iter().cloned())
            .collect()
    }

    fn command(&self, cmd: &mut CommandBuilder, at: Command<'_>) {
        match self {
            Self::Code => {
                for (key, value) in LEAN_ENV {
                    cmd.env(key, value);
                }
            }
            Self::Open => cmd.env("OPENCLAUDE_DISABLE_HEAP_RELAUNCH", "1"),
        }
        cmd.args(args(at.plugin, at.launch));
    }

    fn launch_settings(&self) -> Option<String> {
        Some(settings(AlwaysOn::default()).to_string())
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
                program: WAKE_ENV.into(),
                cause,
            };
            let name = getrandom::u64().map_err(|e| failed(e.to_string()))?;
            let name = format!("{name:016x}.{WAKE_FILE}");
            let (seen, wake) = match at.inside {
                Some(inside) => {
                    let seen = format!("{}/{name}", inside.run_dir());
                    let wake = inside.local(&seen);
                    (seen, wake)
                }
                None => {
                    let wake = at.run_dir.join(name);
                    (wake.display().to_string(), wake)
                }
            };
            touch(Files::of(at.inside), &wake, 0).map_err(failed)?;
            cmd.env(WAKE_ENV, seen);
            let record = Served {
                pid: None,
                distro: None,
            }
            .record(State { wake: wake.clone() });
            let distro = at.inside.map(|inside| inside.distro.clone());
            tokio::spawn(wake_on_mail(wake, distro, port));
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
            if let Some((_, state)) = Served::stored::<State>(rec) {
                tokio::spawn(wake_on_mail(state.wake, rec.distro.clone(), port));
            }
            Ok(())
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
        let grammars: &'static [Grammar] = match self {
            Self::Code => &[PROMPT, PLAN],
            Self::Open => &[
                Grammar {
                    writes: &["No, and tell OpenClaude"],
                    ..PROMPT
                },
                Grammar {
                    writes: &["Tell OpenClaude what to change"],
                    ..PLAN
                },
            ],
        };
        Some((grammars, &DIALOGS))
    }

    fn input_seen(&self, screen: &Screen) -> Option<InputSeen> {
        Some(InputSeen {
            empty: input(&screen.undimmed) == Some(""),
            turn_shown: turn_shown(&screen.text),
        })
    }

    fn install(&self, dir: &Path) -> std::io::Result<()> {
        let manifest = dir.join(".claude-plugin");
        std::fs::create_dir_all(&manifest)?;
        std::fs::write(manifest.join("plugin.json"), MANIFEST)
    }

    fn carry(&self, session: &Session, to: &Carry<'_>) -> Result<(), ResumeError> {
        let site = to.site;
        let file = site.local(&session.0);
        match Files::of(site.inside()).is_file(&file) {
            Ok(true) => {}
            Ok(false) => return Err(ResumeError::NoSession(NO_RECORD.into())),
            Err(e) => return Err(format!("{NO_RECORD}: {e}").into()),
        }
        match (self, place::of(&file)) {
            (Self::Open, _) | (Self::Code, Machine::Ssh(_)) => return Ok(()),
            (Self::Code, Machine::Pc | Machine::Wsl(_)) => {}
        }
        let new_root = site
            .login_dir(&accounts::CLI, to.to)
            .ok_or("홈 폴더를 찾지 못했다".to_owned())?;
        super::copy_session(&file, TRANSCRIPT_DEPTH, &new_root)?;
        let side = file.with_extension("");
        if side.is_dir()
            && let Some(moved) = super::relocate(&side, TRANSCRIPT_DEPTH, &new_root)
            && moved != side
        {
            copy_dir(&side, &moved).map_err(|e| format!("{}: {e}", side.display()))?;
        }
        if let (Some(config), Some(from)) = (
            site.accounts().config_dir(&accounts::CLI, to.to),
            accounts::consent_files(site, to.from),
        ) {
            accounts::carry_consent(&from, &config, &site.cli_cwd(to.cwd))
                .map_err(|e| format!("{}: {e}", config.display()))?;
        }
        Ok(())
    }

    fn end_detached<'a>(
        &'a self,
        rec: &'a Record,
        site: &'a Site,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if let Self::Open = self {
                return Ok(());
            }
            let (Some(session), account) = (&rec.session, rec.account.as_ref()) else {
                return Ok(());
            };
            let Some(out) = run(site.run(&accounts::CLI, &["agents", "--json"], account)).await
            else {
                return Ok(());
            };
            let Some(job) = background_job(out.as_bytes(), &super::session_id(session)) else {
                return Ok(());
            };
            run(site.run(&accounts::CLI, &["stop", &job], account))
                .await
                .map(drop)
                .ok_or_else(|| {
                    format!("`claude stop {job}`이 옛 세션의 백그라운드 작업을 끝내지 못했다")
                })
        })
    }

    fn record<'a>(&'a self, rec: &'a Record) -> BoxFuture<'a, Option<String>> {
        Box::pin(async { rec.session.as_ref().map(|s| s.0.clone()) })
    }
}

#[derive(Serialize, Deserialize)]
struct State {
    wake: PathBuf,
}

fn touch(files: Files<'_>, file: &Path, times: u64) -> Result<(), String> {
    let written = file
        .parent()
        .map_or(Ok(()), |dir| files.create_dir_all(dir))
        .and_then(|()| files.rewrite(file, &times.to_string()));
    written.map_err(|e| format!("{}: {e}", file.display()))
}

fn is_wake_file(path: &Path) -> bool {
    std::env::var_os(WAKE_ENV).is_some_and(|wake| Path::new(&wake).file_name() == path.file_name())
}

async fn wake_on_mail(file: PathBuf, distro: Option<Distro>, port: Arc<dyn Port>) {
    let files = Files::new(distro.as_ref().map(Distro::hosts));
    let mut times = 0;
    let mut on = || {
        times += 1;
        if let Err(e) = touch(files, &file, times) {
            crate::log::logln!("loom: 에이전트를 깨우지 못했다: {e}");
        }
    };
    tokio::select! {
        refused = port.wakes(&mut on) => crate::log::logln!("loom: 에이전트 깨움을 멈췄다: {refused}"),
        () = port.ended() => {}
    }
    if let Err(e) = files.remove(&file) {
        crate::log::logln!("loom: {}을 지우지 못했다: {e}", file.display());
    }
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            super::copy_file(&entry.path(), &target)?;
        }
    }
    Ok(())
}

async fn run(run: Option<Run>) -> Option<String> {
    let run = run?;
    let ran = tokio::task::spawn_blocking(move || run.output(CLI_WAIT))
        .await
        .ok()?
        .ok()?;
    (ran.code == Some(0)).then_some(ran.stdout)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Job {
    id: Option<String>,
    kind: String,
    session_id: String,
}

fn background_job(json: &[u8], session: &str) -> Option<String> {
    let jobs: Vec<Job> = serde_json::from_slice(json).ok()?;
    jobs.into_iter()
        .find(|j| j.kind == "background" && j.session_id == session)?
        .id
}

fn args(plugin: &Path, launch: Launch) -> Vec<String> {
    vec![
        "--append-system-prompt".into(),
        usage(launch),
        "--settings".into(),
        settings(launch.skills).to_string(),
        "--plugin-dir".into(),
        plugin.to_string_lossy().into_owned(),
    ]
}

fn settings(on: AlwaysOn) -> Value {
    let hook = json!([{ "hooks": [{
        "type": "command",
        "command": HOOK,
        "timeout": HOOK_TIMEOUT_SECS,
    }] }]);
    let rewake = json!([{ "hooks": [{
        "type": "command",
        "command": HOOK,
        "asyncRewake": true,
        "timeout": HOOK_TIMEOUT_SECS,
        "rewakeMessage": REWAKE_MESSAGE,
        "rewakeSummary": REWAKE_SUMMARY,
    }] }]);
    let human = HUMAN_NOTIFICATIONS.map(|(name, _)| name).join("|");
    let hidden: serde_json::Map<String, Value> = crate::skills::names(on)
        .map(|name| (name.to_owned(), json!("off")))
        .collect();
    json!({
        "permissions": { "allow": ALLOWED },
        "skillOverrides": hidden,
        "hooks": {
            "SessionStart": hook,
            "UserPromptSubmit": hook,
            "PreToolUse": hook,
            "PostToolUse": hook,
            "PostToolUseFailure": hook,
            "Stop": rewake,
            "StopFailure": hook,
            "FileChanged": rewake,
            "CwdChanged": hook,
            "Notification": [{ "matcher": human, "hooks": hook[0]["hooks"] }],
        },
    })
}

pub const PROMPT: Grammar = Grammar {
    top: Top::PanelEdge,
    options: Options::Numbered,
    guard: Some(GUARD),
    writes: &["No, and tell Claude"],
};

pub const PLAN: Grammar = Grammar {
    top: Top::Line("Ready to code?"),
    options: Options::Numbered,
    guard: None,
    writes: &["Tell Claude what to change"],
};

pub const DIALOGS: [Dialog; 6] = [
    Dialog {
        text: "Choose the text style that looks best with your terminal",
        kind: WaitKind::Auth,
        place: Place::BeforeSession,
        answer: Answer::Below(Options::Listed),
    },
    Dialog {
        text: "Yes, I trust this folder",
        kind: WaitKind::Trust,
        place: Place::BeforeSession,
        answer: Answer::Panel(Options::Listed),
    },
    Dialog {
        text: "Yes, I accept",
        kind: WaitKind::Approval,
        place: Place::BeforeSession,
        answer: Answer::Panel(Options::Listed),
    },
    Dialog {
        text: "Select login method:",
        kind: WaitKind::Auth,
        place: Place::BeforeSession,
        answer: Answer::Below(Options::Numbered),
    },
    Dialog {
        text: "Allow external CLAUDE.md file imports?",
        kind: WaitKind::Trust,
        place: Place::BeforeSession,
        answer: Answer::Panel(Options::Listed),
    },
    Dialog {
        text: "Enter to confirm · Esc to cancel",
        kind: WaitKind::Approval,
        place: Place::OtherBeforeSession,
        answer: Answer::Panel(Options::Listed),
    },
];

const HUMAN_NOTIFICATIONS: [(&str, WaitKind); 2] = [
    ("permission_prompt", WaitKind::Approval),
    ("elicitation_dialog", WaitKind::Question),
];
const IDLE_NOTIFICATION: &str = "idle_prompt";
const ASK_TOOL: &str = "AskUserQuestion";
const ASK_REDIRECT: &str = "loom blocked AskUserQuestion: the human answers only loom ask, and loom keeps no record of that box. Ask the same question with loom ask in the one-line form of the ask-human rules, then end your turn. Do not use AskUserQuestion again.";
const PLAN_TOOL: &str = "ExitPlanMode";

#[derive(Deserialize)]
pub struct Hook {
    #[serde(flatten)]
    pub event: Event,
    agent_id: Option<String>,
    transcript_path: Option<String>,
    last_assistant_message: Option<String>,
    prompt_id: Option<String>,
}

impl HookEvent for Hook {
    fn signals(&self) -> Signals {
        let main = self.agent_id.is_none();
        let wait = self.event.human_wait();
        let wait = ((main && self.event.clears_wait()) || wait.is_some())
            .then(|| Report::HumanWait(wait.map(|(kind, text)| (kind, text, None))));
        let last = self.event.ends_turn() && std::env::var_os(WAKE_ENV).is_some();
        Signals {
            step: self.event.name().to_owned(),
            take: match std::env::var_os(WAKE_ENV) {
                Some(_) if last => Some(Take::Last),
                Some(_) => self.event.take(),
                None => self.event.take_without_wake(),
            }
            .filter(|_| main),
            session: self.transcript_path.clone().map(Session),
            prompt_id: self
                .prompt_id
                .clone()
                .filter(|_| last || self.event.marks_prompt()),
            prompt: self.event.prompt().map(str::to_owned),
            human_input: self.event.prompt().is_some(),
            turn_end: match &self.event {
                Event::Stop => Some(self.last_assistant_message.as_deref().map(turn_reply)),
                _ if last => Some(None),
                _ => None,
            },
            rate_limited: self.event.rate_limited(),
            asks_interrupt: !main && matches!(self.event, Event::PreToolUse { .. }),
            tool: self.event.tool(),
            background: self.event.background().filter(|_| main),
            reports: reported_model(&self.event, self.transcript_path.as_deref())
                .map(Report::Model)
                .into_iter()
                .chain(wait)
                .collect(),
            ..Signals::default()
        }
    }

    fn show(&self, handed: &Handed) -> Shown {
        let shown = self.shown(handed);
        match (&self.event, std::env::var(WAKE_ENV)) {
            (Event::SessionStart { .. } | Event::CwdChanged, Ok(wake)) => Shown {
                out: Some(
                    json!({ "hookSpecificOutput": {
                        "hookEventName": self.event.name(),
                        "watchPaths": [wake],
                    } })
                    .to_string(),
                ),
                ..shown
            },
            _ => shown,
        }
    }
}

impl Hook {
    fn shown(&self, handed: &Handed) -> Shown {
        match handed {
            Handed::Taken { messages, note } => Shown {
                out: self
                    .event
                    .reply(messages, note.as_deref())
                    .map(|reply| reply.to_string()),
                ..Shown::default()
            },
            Handed::Interrupted => Shown {
                out: Some(deny(SUBAGENT_INTERRUPTED.into()).to_string()),
                ..Shown::default()
            },
            Handed::Mail(messages) if messages.is_empty() && self.event.ends_turn() => Shown {
                out: Some(json!({ "continue": false, "stopReason": TURN_ENDED }).to_string()),
                ..Shown::default()
            },
            Handed::Mail(messages) if messages.is_empty() => Shown::default(),
            Handed::Mail(messages) => to_model(super::message_text(messages)),
            Handed::Nudge(text) if self.event.ends_turn() => Shown {
                out: Some(context("PostToolUse", text.clone()).to_string()),
                ..Shown::default()
            },
            Handed::Nudge(text) => to_model(text.clone()),
        }
    }
}

fn turn_reply(text: &str) -> TurnReply {
    match text.trim().strip_prefix(STATUS_MARK) {
        Some(status) => TurnReply::Status {
            status: status.trim().to_owned(),
        },
        None => TurnReply::Answer(text.to_owned()),
    }
}

fn to_model(text: String) -> Shown {
    Shown {
        out: None,
        to_model: Some(text),
        rewake: true,
    }
}

const SUBAGENT_INTERRUPTED: &str =
    "Your parent agent received a loom interrupt. Stop now and return to your parent agent.";

fn deny(text: String) -> Value {
    json!({ "hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "deny",
        "permissionDecisionReason": text,
    } })
}

#[derive(Deserialize)]
#[serde(tag = "hook_event_name")]
pub enum Event {
    SessionStart {
        model: Option<String>,
    },
    UserPromptSubmit {
        #[serde(default)]
        prompt: String,
    },
    PreToolUse {
        tool_name: String,
        tool_input: Value,
    },
    PostToolUse {
        #[serde(default)]
        tool_name: String,
        #[serde(default)]
        tool_input: Value,
        #[serde(default)]
        tool_response: Value,
    },
    PostToolUseFailure,
    Stop,
    StopFailure {
        #[serde(default)]
        error: String,
    },
    Notification {
        #[serde(default)]
        notification_type: String,
        #[serde(default)]
        message: String,
    },
    FileChanged {
        file_path: PathBuf,
    },
    CwdChanged,
}

const NOTIFICATION_TAG: &str = "<task-notification>";
const MONITOR_TOOL: &str = "Monitor";
const TASK_ID_TAG: &str = "<task-id>";
const RATE_LIMIT_ERROR: &str = "rate_limit";
const RATE_LIMIT_TEXT: &str = "Stop and wait for limit to reset";
const TOOL_TARGETS: [&str; 6] = [
    "file_path",
    "notebook_path",
    "command",
    "pattern",
    "url",
    "query",
];
const TOOL_TARGET_CHARS: usize = 80;

fn short_target(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let cwd = std::env::current_dir().ok();
    let line = cwd
        .as_ref()
        .and_then(|dir| line.strip_prefix(dir.to_str()?))
        .and_then(|rest| rest.strip_prefix(['/', '\\']))
        .unwrap_or(line);
    match line.char_indices().nth(TOOL_TARGET_CHARS) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.to_owned(),
    }
}

impl Event {
    pub(super) fn name(&self) -> &'static str {
        match self {
            Event::SessionStart { .. } => "SessionStart",
            Event::UserPromptSubmit { .. } => "UserPromptSubmit",
            Event::PreToolUse { .. } => "PreToolUse",
            Event::PostToolUse { .. } => "PostToolUse",
            Event::PostToolUseFailure => "PostToolUseFailure",
            Event::Stop => "Stop",
            Event::StopFailure { .. } => "StopFailure",
            Event::Notification { .. } => "Notification",
            Event::FileChanged { .. } => "FileChanged",
            Event::CwdChanged => "CwdChanged",
        }
    }

    fn marks_prompt(&self) -> bool {
        matches!(
            self,
            Event::UserPromptSubmit { .. }
                | Event::FileChanged { .. }
                | Event::Stop
                | Event::StopFailure { .. }
        )
    }

    fn ends_turn(&self) -> bool {
        match self {
            Event::PostToolUse {
                tool_name,
                tool_input,
                tool_response,
            } if tool_name == "Bash" && tool_input["run_in_background"] != true => {
                tool_input["command"]
                    .as_str()
                    .and_then(lone_loom_verb)
                    .is_some_and(|verb| TURN_END_VERBS.contains(&verb))
                    && tool_response["stdout"]
                        .as_str()
                        .is_some_and(|out| out.contains(END_TURN_NOW))
            }
            _ => false,
        }
    }

    fn clears_wait(&self) -> bool {
        !matches!(
            self,
            Event::FileChanged { .. } | Event::Notification { .. } | Event::CwdChanged
        )
    }

    pub fn take(&self) -> Option<Take> {
        Some(match self {
            Event::SessionStart { .. } => Take::ListenStart,
            Event::Stop | Event::StopFailure { .. } => Take::ListenEnd,
            Event::UserPromptSubmit { .. } => Take::Now(Kind::ALL),
            Event::PostToolUse { .. } => Take::Now(Kind::ALL),
            Event::PreToolUse { .. } | Event::PostToolUseFailure => Take::Now(&[Kind::Interrupt]),
            Event::FileChanged { file_path } if is_wake_file(file_path) => Take::Woken,
            Event::FileChanged { .. } | Event::Notification { .. } | Event::CwdChanged => {
                return None;
            }
        })
    }

    fn take_without_wake(&self) -> Option<Take> {
        match self {
            Event::SessionStart { .. } | Event::Stop | Event::StopFailure { .. } => {
                Some(Take::WhenIdle)
            }
            Event::Notification {
                notification_type, ..
            } if notification_type == IDLE_NOTIFICATION => Some(Take::WhenIdle),
            event => event.take(),
        }
    }

    pub fn prompt(&self) -> Option<&str> {
        match self {
            Event::UserPromptSubmit { prompt } if !prompt.starts_with(NOTIFICATION_TAG) => {
                Some(prompt)
            }
            _ => None,
        }
    }

    pub fn human_wait(&self) -> Option<(WaitKind, String)> {
        match self {
            Event::PreToolUse { tool_name, .. } if tool_name == PLAN_TOOL => {
                Some((WaitKind::Approval, String::new()))
            }
            Event::Notification {
                notification_type,
                message,
            } => HUMAN_NOTIFICATIONS
                .iter()
                .find(|(name, _)| name == notification_type)
                .map(|(_, kind)| (*kind, message.clone())),
            event if event.rate_limited() => Some((WaitKind::Auth, RATE_LIMIT_TEXT.to_owned())),
            _ => None,
        }
    }

    fn tool(&self) -> Option<String> {
        let Event::PreToolUse {
            tool_name,
            tool_input,
        } = self
        else {
            return None;
        };
        let target = TOOL_TARGETS
            .iter()
            .find_map(|key| tool_input[key].as_str())
            .map(short_target);
        Some(match target {
            Some(target) if !target.is_empty() => format!("{tool_name} · {target}"),
            _ => tool_name.clone(),
        })
    }

    fn background(&self) -> Option<bool> {
        match self {
            Event::PostToolUse {
                tool_name,
                tool_input,
                ..
            } if tool_name == MONITOR_TOOL || tool_input["run_in_background"] == true => Some(true),
            Event::UserPromptSubmit { prompt }
                if prompt.starts_with(NOTIFICATION_TAG) && prompt.contains(TASK_ID_TAG) =>
            {
                Some(false)
            }
            _ => None,
        }
    }

    pub fn rate_limited(&self) -> bool {
        matches!(self, Event::StopFailure { error } if error == RATE_LIMIT_ERROR)
    }

    pub fn reply(&self, taken: &[Delivery], note: Option<&str>) -> Option<Value> {
        let text = super::hook_text(taken, note);
        if let Event::PreToolUse { tool_name, .. } = self
            && [ASK_TOOL, "ask_user_question"].contains(&tool_name.as_str())
        {
            let parts: Vec<String> = [ASK_REDIRECT.to_owned()].into_iter().chain(text).collect();
            return Some(deny(parts.join("\n\n")));
        }
        let Some(text) = text else {
            return match self {
                Event::PreToolUse {
                    tool_name,
                    tool_input,
                } => guarded(tool_name, tool_input).map(|input| {
                    json!({ "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "updatedInput": input,
                    } })
                }),
                _ => None,
            };
        };
        match self {
            Event::SessionStart { .. }
            | Event::Stop
            | Event::StopFailure { .. }
            | Event::Notification { .. }
            | Event::FileChanged { .. }
            | Event::CwdChanged => None,
            Event::PreToolUse { .. } => Some(deny(text)),
            Event::UserPromptSubmit { .. } => Some(context("UserPromptSubmit", text)),
            Event::PostToolUse { .. } => Some(context("PostToolUse", text)),
            Event::PostToolUseFailure => Some(context("PostToolUseFailure", text)),
        }
    }
}

fn reported_model(event: &Event, transcript: Option<&str>) -> Option<Model> {
    if let Event::SessionStart { model, .. } = event {
        return Some(Model {
            model: model.clone(),
            effort: None,
        });
    }
    last_model(&transcript_tail(&PathBuf::from(transcript?))?)
}

fn transcript_tail(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TRANSCRIPT_TAIL_BYTES)))
        .ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    Some(String::from_utf8_lossy(&tail).into_owned())
}

fn last_model(tail: &str) -> Option<Model> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Entry {
        r#type: String,
        #[serde(default)]
        is_sidechain: bool,
        effort: Option<String>,
        message: Option<Message>,
    }
    #[derive(Deserialize)]
    struct Message {
        model: Option<String>,
    }
    tail.lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Entry>(line).ok())
        .filter(|e| e.r#type == "assistant" && !e.is_sidechain)
        .find_map(|e| {
            let model = e.message?.model.filter(|m| m != SYNTHETIC_MODEL)?;
            Some(Model {
                model: Some(model),
                effort: e.effort,
            })
        })
}

fn input(screen: &str) -> Option<&str> {
    let lines: Vec<&str> = screen.lines().map(str::trim_end).collect();
    lines.windows(3).rev().find_map(|w| match w {
        [above, input, below] if is_rule(above) && is_rule(below) => {
            input.strip_prefix(INPUT_MARK).map(str::trim)
        }
        _ => None,
    })
}

fn is_rule(line: &str) -> bool {
    !line.is_empty() && line.chars().all(|c| c == '─')
}

fn context(event: &str, text: String) -> Value {
    json!({ "hookSpecificOutput": { "hookEventName": event, "additionalContext": text } })
}

fn guarded(tool_name: &str, input: &Value) -> Option<Value> {
    if !["Bash", "run_shell_command"].contains(&tool_name)
        || input["run_in_background"] == true
        || input["is_background"] == true
    {
        return None;
    }
    let command = input["command"].as_str()?;
    let mut input = input.clone();
    input["command"] = format!("{GUARD}\n{command}").into();
    Some(input)
}

fn lone_loom_verb(command: &str) -> Option<&str> {
    let command = command
        .strip_prefix(GUARD)
        .map_or(command, |rest| rest.trim_start_matches('\n'))
        .trim();
    let mut quote = None;
    let mut escaped = false;
    for c in command.chars() {
        match (quote, c) {
            _ if escaped => escaped = false,
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), _) => {}
            (_, '\\') => escaped = true,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, ';' | '&' | '|' | '\n' | '`' | '(' | ')' | '<' | '>') => return None,
            (None, _) => {}
        }
    }
    if quote.is_some() || escaped {
        return None;
    }
    let mut words = command.split_whitespace();
    (words.next() == Some(crate::protocol::LOOM))
        .then(|| words.next())
        .flatten()
}

fn turn_shown(screen: &str) -> bool {
    let lines: Vec<&str> = screen.lines().map(str::trim_end).collect();
    let rules: Vec<usize> = (0..lines.len())
        .filter(|&i| !lines[i].is_empty() && lines[i].chars().all(|c| c == '─'))
        .collect();
    let (above, footer) = match rules[..] {
        [.., top, bottom] => (&lines[..top], &lines[bottom + 1..]),
        _ => (&lines[..], &lines[..0]),
    };
    footer.iter().any(|line| line.contains(RUNNING_STATUS)) || above.iter().any(|l| spinner(l))
}

fn spinner(line: &str) -> bool {
    let mut chars = line.chars();
    let glyph = chars
        .next()
        .is_some_and(|c| !c.is_alphanumeric() && !c.is_whitespace() && !"❯●⎿─".contains(c));
    let rest = chars.as_str();
    glyph
        && rest.starts_with(' ')
        && rest[1..]
            .split(' ')
            .next()
            .is_some_and(|word| word.ends_with('…') && word.chars().count() > 1)
}
