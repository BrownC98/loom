use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};

use portable_pty::CommandBuilder;

use super::adapter::{Adapter, Command, Facts, LaunchAt, Logins, Port, Record, Started, TurnTake};
use super::{Handed, HookEvent, ModelRule, Report, Shown, Signals, Take};
use crate::place::Files;
use crate::protocol::{Delivery, Kind, LaunchError, Model, ModelChoice, Session, WaitKind};
use crate::screen::{Answer, Dialog, Grammar, Options, Place, Top};
use crate::skills::{AlwaysOn, Launch, Position};
use crate::usage::UsageProblem;
use crate::wsl::Inside;

mod usage;

pub const PROMPT: Grammar = Grammar {
    top: Top::PanelEdge,
    options: Options::Hinted,
    guard: Some(GUARD),
    writes: &["Skip & tell the agent"],
};

pub const QUESTION: Grammar = Grammar {
    top: Top::Boxed("Question "),
    options: Options::Ticked,
    guard: None,
    writes: &["Other"],
};

pub const DIALOGS: [Dialog; 6] = [
    Dialog {
        text: "Enter next/submit",
        kind: WaitKind::Question,
        place: Place::Panel,
        answer: Answer::Plain,
    },
    Dialog {
        text: "Run this command",
        kind: WaitKind::Approval,
        place: Place::Panel,
        answer: Answer::Plain,
    },
    Dialog {
        text: "Run this MCP tool?",
        kind: WaitKind::Approval,
        place: Place::Panel,
        answer: Answer::Plain,
    },
    Dialog {
        text: "Read this file?",
        kind: WaitKind::Approval,
        place: Place::Panel,
        answer: Answer::Plain,
    },
    Dialog {
        text: "Write to this file?",
        kind: WaitKind::Approval,
        place: Place::Panel,
        answer: Answer::Plain,
    },
    Dialog {
        text: "Delete this file?",
        kind: WaitKind::Approval,
        place: Place::Panel,
        answer: Answer::Plain,
    },
];

const POSITION_ENV: &str = "LOOM_POSITION";
const SKILLS_ENV: &str = "LOOM_SKILLS";
const HOOK_TIMEOUT_SECS: u64 = 10;
const IDLE_HOOK_TIMEOUT_SECS: u64 = 2_000_000;
const HOOKS_FILE: &str = "cursor-hooks.json";
const MANIFEST: &str = r#"{"name":"loom","description":"loom이 싣는 스킬과 메시지 훅","skills":"./skills/","hooks":"./cursor-hooks.json"}"#;
const GUARD: &str = "loom guard";
const TRUST: &str = "--trust";
const AUTH_ENV: [&str; 2] = ["CURSOR_API_KEY", "CURSOR_AUTH_TOKEN"];
const HOOK: &str = "loom hook cursor";
const RULES: &str = "loom hook cursor-rules";
const HELPER_SHELLS: [&str; 2] = [HOOK, RULES];

fn write_plugin(dir: &Path) -> std::io::Result<()> {
    let manifest = dir.join(".cursor-plugin").join("plugin.json");
    std::fs::create_dir_all(manifest.parent().expect("파일 경로에는 부모가 있다"))?;
    std::fs::write(manifest, MANIFEST)?;
    std::fs::write(dir.join(HOOKS_FILE), hooks().to_string())
}

const USER_HOOKS: &str = ".cursor/hooks.json";

const GATE: [(&str, &str); 3] = [
    ("stop", r"printf '{}\n'"),
    ("beforeSubmitPrompt", r#"printf '{"continue":true}\n'"#),
    ("afterAgentResponse", r"printf '{}\n'"),
];

static GATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn user_hooks(inside: Option<&Inside>) -> Option<PathBuf> {
    match inside {
        Some(inside) => Some(inside.local(&format!("{}/{USER_HOOKS}", inside.home()))),
        None if cfg!(windows) => None,
        None => std::env::home_dir().map(|home| home.join(USER_HOOKS)),
    }
}

fn open_gate(files: Files, path: &Path) -> std::io::Result<()> {
    let _held = GATE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let text = match files.read(path) {
        Ok(bytes) => Some(String::from_utf8(bytes).map_err(std::io::Error::other)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let Some(gated) = with_gate(text.as_deref())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?
    else {
        return Ok(());
    };
    files.write(path, gated.as_bytes())
}

fn with_gate(text: Option<&str>) -> Result<Option<String>, String> {
    let mut config: Value = match text {
        Some(text) => serde_json::from_str(text).map_err(|e| e.to_string())?,
        None => json!({ "version": 1 }),
    };
    let root = config
        .as_object_mut()
        .ok_or("the hooks file is not a JSON object")?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("\"hooks\" is not a JSON object")?;
    let mut changed = false;
    for (event, command) in GATE {
        let entries = hooks.entry(event).or_insert_with(|| json!([]));
        if entries.as_array().is_some_and(Vec::is_empty) {
            *entries = json!([{ "command": command, "timeout": HOOK_TIMEOUT_SECS }]);
            changed = true;
        }
    }
    Ok(changed.then(|| format!("{config:#}\n")))
}

fn hooks() -> Value {
    let hook = json!([{ "command": HOOK, "timeout": HOOK_TIMEOUT_SECS }]);
    json!({
        "version": 1,
        "hooks": {
            "sessionStart": [
                { "command": HOOK, "timeout": HOOK_TIMEOUT_SECS },
                { "command": RULES, "timeout": HOOK_TIMEOUT_SECS },
            ],
            "beforeSubmitPrompt": hook,
            "preToolUse": hook,
            "postToolUse": hook,
            "postToolUseFailure": hook,
            "afterAgentResponse": hook,
            "stop": [{ "command": HOOK, "timeout": IDLE_HOOK_TIMEOUT_SECS, "loop_limit": null }],
        },
    })
}

pub struct Cursor;

impl Adapter for Cursor {
    fn facts(&self) -> Facts {
        Facts {
            logins: Logins::One(&Cursor),
            sleeps: true,
            listens_at_start: false,
            turn_take: TurnTake::StaysInTurn,
            model_rule: ModelRule::EffortNeedsModel,
            pinned: None,
            usage_part: "usage/cursor",
            waiting: WAITING,
            helper_shells: &HELPER_SHELLS,
        }
    }

    fn leading_args(&self, resume: Option<&Session>, bypass: &[String]) -> Vec<String> {
        resume
            .into_iter()
            .flat_map(|session| ["--resume".into(), super::session_id(session)])
            .chain([TRUST.into()])
            .chain(bypass.iter().cloned())
            .collect()
    }

    fn launch<'a>(
        &'a self,
        at: &'a LaunchAt<'_>,
        _: Option<&'a Session>,
        _: &'a [String],
        _: &'a mut CommandBuilder,
        _: Arc<dyn Port>,
    ) -> BoxFuture<'a, Result<Started, LaunchError>> {
        let status = (!env_auth(at)).then(|| usage::status(at.site));
        let distro = at.site.distro().map(str::to_owned);
        Box::pin(async move {
            let Some(status) = status else {
                return Ok(Started::default());
            };
            let check = || usage::STATUS.into();
            match tokio::task::spawn_blocking(status).await {
                Ok(Ok(usage::Status {
                    is_authenticated: true,
                    ..
                })) => Ok(Started::default()),
                Ok(Ok(_)) => Err(LaunchError::NotLoggedIn {
                    command: usage::COMMAND.into(),
                    distro,
                }),
                Ok(Err(UsageProblem::NotInstalled(command))) => Err(LaunchError::NotInstalled {
                    agent: usage::AGENT.into(),
                    command,
                    distro,
                }),
                Ok(Err(UsageProblem::NoReply(_))) => Err(LaunchError::LoginCheckNoReply {
                    command: check(),
                    distro,
                    secs: crate::usage::PROBE_TIMEOUT.as_secs(),
                }),
                Ok(Err(_)) | Err(_) => Err(LaunchError::LoginCheckUnreadable {
                    command: check(),
                    distro,
                }),
            }
        })
    }

    fn prepare(&self, at: &LaunchAt<'_>) -> std::io::Result<()> {
        if let Some(hooks) = user_hooks(at.inside)
            && let Err(e) = open_gate(Files::of(at.inside), &hooks)
        {
            eprintln!(
                "loom: {}에 쉬는 대기 훅을 넣지 못했다: {e}",
                hooks.display()
            );
        }
        Ok(())
    }

    fn command(&self, cmd: &mut CommandBuilder, at: Command<'_>) {
        cmd.args([
            "--plugin-dir".into(),
            at.plugin.to_string_lossy().into_owned(),
        ]);
        cmd.env(POSITION_ENV, position_value(at.launch.position));
        cmd.env(SKILLS_ENV, skills_value(at.launch.skills));
    }

    fn choice_args(&self, choice: &ModelChoice) -> Option<Vec<String>> {
        let model = match (&choice.model, &choice.effort) {
            (Some(model), Some(effort)) => format!("{model}-{effort}"),
            (Some(model), None) => model.clone(),
            (None, _) => return None,
        };
        Some(vec!["--model".into(), model])
    }

    fn screen(&self) -> Option<(&'static [Grammar], &'static [Dialog])> {
        Some((&[PROMPT, QUESTION], &DIALOGS))
    }

    fn install(&self, dir: &Path) -> std::io::Result<()> {
        write_plugin(dir)
    }

    fn record<'a>(&'a self, rec: &'a Record) -> BoxFuture<'a, Option<String>> {
        Box::pin(async { rec.session.as_ref().map(|s| s.0.clone()) })
    }
}

fn env_auth(at: &LaunchAt<'_>) -> bool {
    AUTH_ENV.iter().any(|key| {
        let given = at.env.iter().rev().find(|(k, _)| k == key);
        let inherited = || at.inside.is_none().then(|| std::env::var_os(key)).flatten();
        given
            .map(|(_, v)| v.clone())
            .or_else(inherited)
            .is_some_and(|v| !v.is_empty())
    })
}

fn position_value(position: Position) -> &'static str {
    match position {
        Position::Root => "root",
        Position::Child => "child",
    }
}

fn position_from(value: Option<&str>) -> Position {
    match value {
        Some("root") => Position::Root,
        _ => Position::Child,
    }
}

pub fn skills_value(skills: AlwaysOn) -> String {
    serde_json::to_string(&skills).expect("스위치는 늘 직렬화된다")
}

fn skills_from(value: Option<&str>) -> AlwaysOn {
    value
        .and_then(|v| serde_json::from_str(v).ok())
        .unwrap_or_default()
}

pub(super) const WAITING: &str = "When no independent work remains, end your turn. loom wakes you with a new message when one arrives. Cursor runs that wake hook only when ~/.cursor/hooks.json has a \"stop\" entry; if it has none, wait for an expected reply with loom wait --reply-to <message-id> --timeout <seconds> before you end your turn. The loom guard line before shell commands is inserted by loom; do not remove it or add it yourself. Do not use your AskQuestion tool: the human answers only loom ask.";

const QUESTION_TOOL: &str = "AskQuestion";

pub fn used_question_box(transcript: impl std::io::BufRead) -> std::io::Result<bool> {
    let mut used = false;
    for line in transcript.split(b'\n') {
        let line = line?;
        let line = String::from_utf8_lossy(&line);
        if !line.contains("\"user\"") && !line.contains(QUESTION_TOOL) {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if entry["role"] == "user" {
            used = false;
        } else if entry["message"]["content"].as_array().is_some_and(|parts| {
            parts
                .iter()
                .any(|part| part["type"] == "tool_use" && part["name"] == QUESTION_TOOL)
        }) {
            used = true;
        }
    }
    Ok(used)
}

pub fn usage() -> String {
    format!("{}\n{WAITING}", super::COMMUNICATION_USAGE)
}

pub struct Rules(Launch);

impl Rules {
    pub fn from_env() -> Self {
        let env = |key| std::env::var(key).ok();
        Rules(Launch {
            position: position_from(env(POSITION_ENV).as_deref()),
            skills: skills_from(env(SKILLS_ENV).as_deref()),
        })
    }
}

impl HookEvent for Rules {
    fn signals(&self) -> Signals {
        Signals::default()
    }

    fn show(&self, _: &Handed) -> Shown {
        let rules = json!({ "additional_context": crate::skills::always_on(self.0) });
        Shown {
            out: Some(rules.to_string()),
            ..Shown::default()
        }
    }
}

pub struct Hook {
    event: Event,
    input: Value,
}

impl Hook {
    pub fn parse(input: Value) -> Result<Self, serde_json::Error> {
        Ok(Self {
            event: Event::deserialize(&input)?,
            input,
        })
    }

    fn used_question_box(&self) -> std::io::Result<bool> {
        let Some(path) = self.input["transcript_path"].as_str() else {
            return Ok(false);
        };
        let here = std::env::current_dir().unwrap_or_default();
        let Ok(file) = std::fs::File::open(crate::place::from_node(path.into(), &here)) else {
            return Ok(false);
        };
        used_question_box(std::io::BufReader::new(file))
    }
}

impl HookEvent for Hook {
    fn signals(&self) -> Signals {
        let text = |key: &str| self.input[key].as_str().map(str::to_owned);
        let turn = text("generation_id").filter(|id| !id.is_empty());
        let notice = match &self.event {
            Event::AfterAgentResponse { text } => Some(Report::Said {
                text: text.clone(),
                turn: turn.clone(),
            }),
            Event::Stop {
                status: StopStatus::Aborted | StopStatus::Error,
                ..
            } => Some(Report::TurnStopped { turn: turn.clone() }),
            Event::Stop {
                status: StopStatus::Completed,
                ..
            } => match self.used_question_box() {
                Ok(true) => Some(Report::QuestionBox),
                Ok(false) => None,
                Err(e) => {
                    eprintln!("loom: could not read the transcript: {e}");
                    None
                }
            },
            _ => None,
        };
        Signals {
            step: text("hook_event_name").unwrap_or_default(),
            take: self.event.take(),
            session: text("transcript_path").map(Session),
            human_input: matches!(self.event, Event::BeforeSubmitPrompt { .. }),
            turn,
            turn_end: matches!(
                self.event,
                Event::Stop {
                    status: StopStatus::Completed,
                    ..
                }
            )
            .then_some(None),
            reports: self
                .event
                .model()
                .map(Report::Model)
                .into_iter()
                .chain(notice)
                .collect(),
            ..Signals::default()
        }
    }

    fn show(&self, handed: &Handed) -> Shown {
        let out = match handed {
            Handed::Taken { messages, note } => self.event.reply(messages, note.as_deref()),
            Handed::Mail(messages) => self.event.reply(messages, None),
            Handed::Nudge(text) => json!({ "followup_message": text }),
            Handed::Interrupted => self.event.reply(&[], None),
        };
        Shown {
            out: Some(out.to_string()),
            ..Shown::default()
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "hook_event_name", rename_all = "camelCase")]
pub enum Event {
    SessionStart {
        model: Option<String>,
    },
    BeforeSubmitPrompt {
        model: Option<String>,
    },
    PreToolUse {
        #[serde(flatten)]
        call: ToolCall,
    },
    PostToolUse {
        #[serde(flatten)]
        call: ToolCall,
        duration: Option<f64>,
    },
    PostToolUseFailure {
        #[serde(flatten)]
        call: ToolCall,
        #[serde(default)]
        is_interrupt: bool,
    },
    Stop {
        model: Option<String>,
        #[serde(default)]
        status: StopStatus,
    },
    AfterAgentResponse {
        #[serde(default)]
        text: String,
    },
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum StopStatus {
    Completed,
    Aborted,
    #[default]
    #[serde(other)]
    Error,
}

#[derive(Deserialize)]
pub struct ToolCall {
    tool_name: String,
    #[serde(default)]
    tool_input: Value,
    transcript_path: Option<String>,
}

const SEEN_TOOLS: [&str; 5] = ["Shell", "Read", "Write", "Grep", "Task"];
const POLLED_FOLDER: &str = "terminals";

impl ToolCall {
    fn reaches_model(&self) -> bool {
        self.transcript_path.is_some()
            && SEEN_TOOLS.contains(&self.tool_name.as_str())
            && !(self.tool_name == "Read" && self.polled_file())
    }

    fn polled_file(&self) -> bool {
        self.tool_input["file_path"]
            .as_str()
            .and_then(|path| Path::new(path).parent()?.file_name())
            .is_some_and(|folder| folder == POLLED_FOLDER)
    }

    fn sent_to_background(&self, duration: Option<f64>) -> bool {
        self.tool_name == "Shell"
            && duration
                .zip(self.tool_input["timeout"].as_f64())
                .is_some_and(|(duration, timeout)| duration >= timeout)
    }
}

impl Event {
    pub fn take(&self) -> Option<Take> {
        Some(match self {
            Event::SessionStart { .. } | Event::AfterAgentResponse { .. } => return None,
            Event::Stop {
                status: StopStatus::Completed,
                ..
            } => Take::WhenIdle,
            Event::Stop { .. } => return None,
            Event::BeforeSubmitPrompt { .. } => Take::Now(Kind::ALL),
            Event::PostToolUse { call, duration } => {
                if !call.reaches_model() || call.sent_to_background(*duration) {
                    return None;
                }
                Take::Now(Kind::ALL)
            }
            Event::PostToolUseFailure {
                is_interrupt: true, ..
            } => return None,
            Event::PreToolUse { call } | Event::PostToolUseFailure { call, .. } => {
                if !call.reaches_model() {
                    return None;
                }
                Take::Now(&[Kind::Interrupt])
            }
        })
    }

    pub fn model(&self) -> Option<Model> {
        match self {
            Event::SessionStart { model }
            | Event::BeforeSubmitPrompt { model, .. }
            | Event::Stop { model, .. } => Some(Model {
                model: model.clone(),
                effort: None,
            }),
            _ => None,
        }
    }

    pub fn reply(&self, taken: &[Delivery], note: Option<&str>) -> Value {
        let Some(text) = super::hook_text(taken, note) else {
            return match self {
                Event::SessionStart { .. } => json!({ "additional_context": usage() }),
                Event::BeforeSubmitPrompt { .. } => json!({ "continue": true }),
                Event::PreToolUse { call } => match guarded(&call.tool_name, &call.tool_input) {
                    Some(input) => json!({ "updated_input": input }),
                    None => json!({}),
                },
                _ => json!({}),
            };
        };
        match self {
            Event::SessionStart { .. } => json!({ "additional_context": usage() }),
            Event::Stop { .. } => json!({ "followup_message": text }),
            Event::BeforeSubmitPrompt { .. }
            | Event::PostToolUse { .. }
            | Event::PostToolUseFailure { .. } => {
                json!({ "continue": true, "additional_context": text })
            }
            Event::AfterAgentResponse { .. } => json!({}),
            Event::PreToolUse { .. } => json!({
                "permission": "deny",
                "user_message": "loom message",
                "additional_context": text,
            }),
        }
    }
}

fn guarded(tool_name: &str, input: &Value) -> Option<Value> {
    if tool_name != "Shell" || input["is_background"] == true {
        return None;
    }
    let command = input["command"].as_str()?;
    let mut input = input.clone();
    input["command"] = format!("{GUARD}\n{command}").into();
    Some(input)
}
