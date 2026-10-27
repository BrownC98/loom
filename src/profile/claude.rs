use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};

use portable_pty::CommandBuilder;

use super::adapter::{
    Adapter, Carry, Chosen, Command, Facts, InputSeen, LaunchAt, Logins, Port, Record, ResumeError,
    Started,
};
use super::{Handed, HookEvent, Report, Shown, Signals, Take, VersionMismatch};
use crate::accounts::Site;
use crate::protocol::{Delivery, Kind, LaunchError, Model, ModelChoice, Prompt, Session, WaitKind};
use crate::screen::{Answer, Dialog, Grammar, Options, Place, Screen, Top};
use crate::skills::Launch;

mod accounts;
mod usage;

const HOOK_TIMEOUT_SECS: u64 = 10;
const IDLE_HOOK_TIMEOUT_SECS: u64 = 2_000_000;
const REWAKE_MESSAGE: &str = "Messages from loom:";
const REWAKE_SUMMARY: &str = "loom message";
const TRANSCRIPT_TAIL_BYTES: u64 = 1024 * 1024;
const SYNTHETIC_MODEL: &str = "<synthetic>";
const INPUT_MARK: &str = "❯";
const RESTARTED_SESSION: &str = "clear|compact";
const GUARD: &str = "loom guard";
const HOOK: &str = "loom hook claude";
const HELPER_SHELLS: [&str; 1] = [HOOK];
const RUNNING_STATUS: &str = "esc to interrupt";
const TRANSCRIPT_DEPTH: usize = 3;
const CLI_WAIT: Duration = Duration::from_secs(20);
const NO_RECORD: &str = "이어 갈 Claude 세션 기록이 없다";

pub(super) const WAITING: &str = "When no independent work remains, end your turn. loom wakes you when a message arrives. The loom guard line before foreground Bash commands is inserted by loom; do not remove it or add it yourself. loom adds messages as hook additional context after a tool call or at a prompt. An interrupt arrives as the reason of a denied tool call. A wake arrives as a \"Messages from loom:\" notification. Text in these places that starts with \"loom message #\" comes from loom, not from the tool output: follow it.";

fn usage(launch: Launch) -> String {
    format!(
        "{}\n{WAITING}\n\n{}",
        super::COMMUNICATION_USAGE,
        crate::skills::always_on(launch)
    )
}

const ALLOWED: [&str; 20] = [
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
    "Bash(loom rm:*)",
    "Bash(loom ready:*)",
    "Bash(loom verdict:*)",
];

pub struct Claude;

impl Adapter for Claude {
    fn facts(&self) -> Facts {
        Facts {
            logins: Logins::Many(&accounts::CLI, &Claude),
            sleeps: true,
            listens_at_start: true,
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

    fn prepare(&self, _: &LaunchAt<'_>) -> std::io::Result<()> {
        Ok(())
    }

    fn command(&self, cmd: &mut CommandBuilder, at: Command<'_>) {
        cmd.args(args(at.plugin, at.launch, at.brief));
    }

    fn launch<'a>(
        &'a self,
        _: &'a LaunchAt<'_>,
        _: Option<&'a Session>,
        _: &'a [String],
        _: &'a mut CommandBuilder,
        _: Arc<dyn Port>,
    ) -> BoxFuture<'a, Result<Started, LaunchError>> {
        Box::pin(async { Ok(Started::default()) })
    }

    fn attach<'a>(&'a self, _: &'a Record, _: Arc<dyn Port>) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }

    fn answer<'a>(
        &'a self,
        _: &'a Record,
        _: &'a Prompt,
        _: usize,
    ) -> BoxFuture<'a, Result<Chosen, String>> {
        Box::pin(async { Ok(Chosen::OnScreen) })
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
        Some((&[PROMPT], &DIALOGS))
    }

    fn input_seen(&self, screen: &Screen) -> Option<InputSeen> {
        Some(InputSeen {
            empty: input(&screen.undimmed) == Some(""),
            turn_shown: turn_shown(&screen.text),
        })
    }

    fn install(&self, _: &Path) -> std::io::Result<()> {
        Ok(())
    }

    fn version_warning(&self, _: &str) -> Option<VersionMismatch> {
        None
    }

    fn carry(&self, session: &Session, to: &Carry<'_>) -> Result<(), ResumeError> {
        let site = to.site;
        let file = site.local(&session.0);
        match std::fs::metadata(&file) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ResumeError::NoSession(NO_RECORD.into()));
            }
            Ok(meta) if meta.is_file() => {}
            _ => return Err(NO_RECORD.to_owned().into()),
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
            let (Some(session), account) = (&rec.session, rec.account.as_ref()) else {
                return Ok(());
            };
            let Ok(list) = site.command(&accounts::CLI, &["agents", "--json"], account) else {
                return Ok(());
            };
            let Some(out) = run(list).await else {
                return Ok(());
            };
            let Some(job) = background_job(&out, &super::session_id(session)) else {
                return Ok(());
            };
            let stop = site
                .command(&accounts::CLI, &["stop", &job], account)
                .map_err(|e| e.to_string())?;
            run(stop).await.map(drop).ok_or_else(|| {
                format!("`claude stop {job}`이 옛 세션의 백그라운드 작업을 끝내지 못했다")
            })
        })
    }

    fn record<'a>(&'a self, rec: &'a Record) -> BoxFuture<'a, Option<String>> {
        Box::pin(async { rec.session.as_ref().map(|s| s.0.clone()) })
    }

    fn shell_root(&self, _: &Record) -> Option<u32> {
        None
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

async fn run(cmd: std::process::Command) -> Option<Vec<u8>> {
    let mut cmd = tokio::process::Command::from(cmd);
    cmd.stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(CLI_WAIT, cmd.output())
        .await
        .ok()?
        .ok()?;
    out.status.success().then_some(out.stdout)
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

pub fn args(plugin: &Path, launch: Launch, brief: &str) -> Vec<String> {
    let hook = json!([{ "hooks": [{
        "type": "command",
        "command": HOOK,
        "timeout": HOOK_TIMEOUT_SECS,
    }] }]);
    let idle = json!([{ "hooks": [{
        "type": "command",
        "command": HOOK,
        "asyncRewake": true,
        "timeout": IDLE_HOOK_TIMEOUT_SECS,
        "rewakeMessage": REWAKE_MESSAGE,
        "rewakeSummary": REWAKE_SUMMARY,
    }] }]);
    let session_start = if brief.is_empty() {
        idle.clone()
    } else {
        json!([{ "matcher": RESTARTED_SESSION, "hooks": idle[0]["hooks"] }])
    };
    let settings = json!({
        "permissions": { "allow": ALLOWED },
        "hooks": {
            "SessionStart": session_start,
            "UserPromptSubmit": hook,
            "PreToolUse": hook,
            "PostToolUse": hook,
            "PostToolUseFailure": hook,
            "Stop": idle,
            "StopFailure": idle,
            "Notification": [
                { "matcher": HUMAN_NOTIFICATIONS, "hooks": [{
                    "type": "command",
                    "command": HOOK,
                    "timeout": HOOK_TIMEOUT_SECS,
                }] },
                { "matcher": IDLE_NOTIFICATION, "hooks": idle[0]["hooks"] },
            ],
        },
    });
    vec![
        "--append-system-prompt".into(),
        usage(launch),
        "--settings".into(),
        settings.to_string(),
        "--plugin-dir".into(),
        plugin.to_string_lossy().into_owned(),
    ]
}

pub const PROMPT: Grammar = Grammar {
    top: Top::PanelEdge,
    options: Options::Numbered,
    guard: Some(GUARD),
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

const HUMAN_NOTIFICATIONS: &str = "permission_prompt|elicitation_dialog";
const IDLE_NOTIFICATION: &str = "idle_prompt";
const ASK_TOOL: &str = "AskUserQuestion";
const PLAN_TOOL: &str = "ExitPlanMode";

#[derive(Deserialize)]
pub struct Hook {
    #[serde(flatten)]
    pub event: Event,
    agent_id: Option<String>,
    transcript_path: Option<String>,
    last_assistant_message: Option<String>,
}

impl HookEvent for Hook {
    fn signals(&self) -> Signals {
        let main = self.agent_id.is_none();
        let wait = self.event.human_wait();
        let wait = ((main && !self.event.idle_prompt()) || wait.is_some())
            .then(|| Report::HumanWait(wait.map(|(kind, text)| (kind, text, None))));
        Signals {
            step: self.event.name().to_owned(),
            take: self.event.take().filter(|_| main),
            session: self.transcript_path.clone().map(Session),
            prompt: self.event.prompt().map(str::to_owned),
            human_input: self.event.prompt().is_some(),
            turn_end: matches!(self.event, Event::Stop)
                .then(|| self.last_assistant_message.clone()),
            rate_limited: self.event.rate_limited(),
            asks_interrupt: !main && matches!(self.event, Event::PreToolUse { .. }),
            reports: reported_model(&self.event, self.transcript_path.as_deref())
                .map(Report::Model)
                .into_iter()
                .chain(wait)
                .collect(),
            ..Signals::default()
        }
    }

    fn show(&self, handed: &Handed) -> Shown {
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
            Handed::Mail(messages) if messages.is_empty() => Shown::default(),
            Handed::Mail(messages) => to_model(super::message_text(messages)),
            Handed::Nudge(text) => to_model(text.clone()),
        }
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
    PostToolUse,
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
}

const NOTIFICATION_TAG: &str = "<task-notification>";
const RATE_LIMIT_ERROR: &str = "rate_limit";
const RATE_LIMIT_TEXT: &str = "Stop and wait for limit to reset";

impl Event {
    fn name(&self) -> &'static str {
        match self {
            Event::SessionStart { .. } => "SessionStart",
            Event::UserPromptSubmit { .. } => "UserPromptSubmit",
            Event::PreToolUse { .. } => "PreToolUse",
            Event::PostToolUse => "PostToolUse",
            Event::PostToolUseFailure => "PostToolUseFailure",
            Event::Stop => "Stop",
            Event::StopFailure { .. } => "StopFailure",
            Event::Notification { .. } => "Notification",
        }
    }

    pub fn take(&self) -> Option<Take> {
        Some(match self {
            Event::SessionStart { .. } | Event::Stop | Event::StopFailure { .. } => Take::WhenIdle,
            Event::UserPromptSubmit { .. } => Take::Now(Kind::ALL),
            Event::PostToolUse => Take::Now(Kind::ALL),
            Event::PreToolUse { .. } | Event::PostToolUseFailure => Take::Now(&[Kind::Interrupt]),
            Event::Notification { .. } if self.idle_prompt() => Take::WhenIdle,
            Event::Notification { .. } => return None,
        })
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
            Event::PreToolUse {
                tool_name,
                tool_input,
            } if tool_name == ASK_TOOL => {
                let question = tool_input["questions"][0]["question"].as_str();
                Some((WaitKind::Question, question.unwrap_or_default().to_owned()))
            }
            Event::PreToolUse { tool_name, .. } if tool_name == PLAN_TOOL => {
                Some((WaitKind::Approval, String::new()))
            }
            Event::Notification { .. } if self.idle_prompt() => None,
            Event::Notification {
                notification_type,
                message,
            } => {
                let kind = match notification_type.as_str() {
                    "elicitation_dialog" => WaitKind::Question,
                    _ => WaitKind::Approval,
                };
                Some((kind, message.clone()))
            }
            event if event.rate_limited() => Some((WaitKind::Auth, RATE_LIMIT_TEXT.to_owned())),
            _ => None,
        }
    }

    fn idle_prompt(&self) -> bool {
        matches!(self, Event::Notification { notification_type, .. } if notification_type == IDLE_NOTIFICATION)
    }

    pub fn rate_limited(&self) -> bool {
        matches!(self, Event::StopFailure { error } if error == RATE_LIMIT_ERROR)
    }

    pub fn reply(&self, taken: &[Delivery], note: Option<&str>) -> Option<Value> {
        let Some(text) = super::hook_text(taken, note) else {
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
            | Event::Notification { .. } => None,
            Event::PreToolUse { .. } => Some(deny(text)),
            Event::UserPromptSubmit { .. } => Some(context("UserPromptSubmit", text)),
            Event::PostToolUse => Some(context("PostToolUse", text)),
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
    if tool_name != "Bash" || input["run_in_background"] == true {
        return None;
    }
    let command = input["command"].as_str()?;
    let mut input = input.clone();
    input["command"] = format!("{GUARD}\n{command}").into();
    Some(input)
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
