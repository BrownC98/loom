use std::path::Path;

use serde::Deserialize;
use serde_json::{Value, json};

use super::Take;
use crate::graph::{Delivery, Kind, Model, WaitKind};
use crate::screen::{Dialog, Grammar, Options, Place, Top};
use crate::skills::{AlwaysOn, Launch, Position};

pub const PROMPT: Grammar = Grammar {
    top: Top::PanelEdge,
    options: Options::Hinted,
    guard: Some(GUARD),
};

pub const PROMPTS: [Grammar; 2] = [
    PROMPT,
    Grammar {
        top: Top::Boxed("Do you trust the contents of this directory?"),
        options: Options::Bracketed,
        guard: None,
    },
];

pub const DIALOGS: [Dialog; 8] = [
    Dialog {
        text: "Enter next/submit",
        kind: WaitKind::Question,
        place: Place::Panel,
    },
    Dialog {
        text: "Do you trust the contents of this directory?",
        kind: WaitKind::Trust,
        place: Place::BeforeSession,
    },
    Dialog {
        text: "Run this command",
        kind: WaitKind::Approval,
        place: Place::Panel,
    },
    Dialog {
        text: "Run this MCP tool?",
        kind: WaitKind::Approval,
        place: Place::Panel,
    },
    Dialog {
        text: "Read this file?",
        kind: WaitKind::Approval,
        place: Place::Panel,
    },
    Dialog {
        text: "Write to this file?",
        kind: WaitKind::Approval,
        place: Place::Panel,
    },
    Dialog {
        text: "Delete this file?",
        kind: WaitKind::Approval,
        place: Place::Panel,
    },
    Dialog {
        text: "Press any key to log in",
        kind: WaitKind::Auth,
        place: Place::BeforeSession,
    },
];

pub const POSITION_ENV: &str = "LOOM_POSITION";
pub const SKILLS_ENV: &str = "LOOM_SKILLS";
#[cfg(test)]
const CONTEXT_LIMIT_CHARS: usize = 10_000;
pub const HOOK_LIMIT_BYTES: usize = super::BATCH_LIMIT_BYTES;
const HOOK_TIMEOUT_SECS: u64 = 10;
const IDLE_HOOK_TIMEOUT_SECS: u64 = 2_000_000;
const HOOKS_FILE: &str = "cursor-hooks.json";
const MANIFEST: &str = r#"{"name":"loom","description":"loom이 싣는 스킬과 메시지 훅","skills":"./skills/","hooks":"./cursor-hooks.json"}"#;
pub const RESUMED_PROMPT: &str = "loom relaunched your process. If your last turn was cut, continue it. Otherwise end your turn.";
const GUARD: &str = "loom guard";

pub fn write_plugin(dir: &Path) -> std::io::Result<()> {
    let manifest = dir.join(".cursor-plugin").join("plugin.json");
    std::fs::create_dir_all(manifest.parent().expect("파일 경로에는 부모가 있다"))?;
    std::fs::write(manifest, MANIFEST)?;
    std::fs::write(dir.join(HOOKS_FILE), hooks().to_string())
}

pub const USER_HOOKS: &str = ".cursor/hooks.json";

const GATE: [(&str, &str); 3] = [
    ("stop", r"printf '{}\n'"),
    ("beforeSubmitPrompt", r#"printf '{"continue":true}\n'"#),
    ("afterAgentResponse", r"printf '{}\n'"),
];

static GATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn open_gate(path: &Path) -> std::io::Result<()> {
    let _held = GATE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let target = std::fs::canonicalize(path);
    let path = target.as_deref().unwrap_or(path);
    let text = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let Some(gated) = with_gate(text.as_deref())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?
    else {
        return Ok(());
    };
    std::fs::create_dir_all(path.parent().expect("파일 경로에는 부모가 있다"))?;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let draft = path.with_extension(format!(
        "json.{}.{}.loom",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let written = std::fs::write(&draft, gated)
        .and_then(|()| match std::fs::metadata(path) {
            Ok(old) => std::fs::set_permissions(&draft, old.permissions()),
            Err(_) => Ok(()),
        })
        .and_then(|()| std::fs::rename(&draft, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&draft);
    }
    written
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
    let hook = json!([{ "command": "loom cursor-hook", "timeout": HOOK_TIMEOUT_SECS }]);
    json!({
        "version": 1,
        "hooks": {
            "sessionStart": [
                { "command": "loom cursor-hook", "timeout": HOOK_TIMEOUT_SECS },
                { "command": "loom cursor-rules", "timeout": HOOK_TIMEOUT_SECS },
            ],
            "beforeSubmitPrompt": hook,
            "preToolUse": hook,
            "postToolUse": hook,
            "postToolUseFailure": hook,
            "afterAgentResponse": hook,
            "stop": [{ "command": "loom cursor-hook", "timeout": IDLE_HOOK_TIMEOUT_SECS, "loop_limit": null }],
        },
    })
}

pub fn args(plugin: &Path) -> Vec<String> {
    vec!["--plugin-dir".into(), plugin.to_string_lossy().into_owned()]
}

pub fn position_value(position: Position) -> &'static str {
    match position {
        Position::Root => "root",
        Position::Child => "child",
    }
}

pub fn position_from(value: Option<&str>) -> Position {
    match value {
        Some("root") => Position::Root,
        _ => Position::Child,
    }
}

pub fn skills_value(skills: AlwaysOn) -> String {
    serde_json::to_string(&skills).expect("스위치는 늘 직렬화된다")
}

pub fn skills_from(value: Option<&str>) -> AlwaysOn {
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

pub fn rules(launch: Launch) -> Value {
    json!({ "additional_context": crate::skills::always_on(launch) })
}

pub fn choice_args(choice: &super::ModelChoice) -> Option<Vec<String>> {
    let model = match (&choice.model, &choice.effort) {
        (Some(model), Some(effort)) => format!("{model}-{effort}"),
        (Some(model), None) => model.clone(),
        (None, _) => return None,
    };
    Some(vec!["--model".into(), model])
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
    },
    Stop {
        model: Option<String>,
    },
    AfterAgentResponse {
        #[serde(default)]
        text: String,
    },
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
            Event::Stop { .. } => Take::WhenIdle,
            Event::BeforeSubmitPrompt { .. } => Take::Now(Kind::ALL),
            Event::PostToolUse { call, duration } => {
                if !call.reaches_model() || call.sent_to_background(*duration) {
                    return None;
                }
                Take::Now(Kind::ALL)
            }
            Event::PreToolUse { call } | Event::PostToolUseFailure { call } => {
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
            | Event::Stop { model } => Some(Model {
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

#[cfg(test)]
mod tests {
    #[test]
    fn only_a_question_tool_call_in_the_last_turn_counts() {
        let user =
            r#"{"role":"user","message":{"content":[{"type":"text","text":"AskQuestion"}]}}"#;
        let asked = r#"{"role":"assistant","message":{"content":[{"type":"text","text":"x"},{"type":"tool_use","name":"AskQuestion","input":{}}]}}"#;
        let read = r#"{"role":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{}}]}}"#;
        let ended = r#"{"type":"turn_ended","status":"success"}"#;
        let used = |lines: &[&str]| super::used_question_box(lines.join("\n").as_bytes()).unwrap();
        assert!(used(&[user, asked, read, ended]));
        assert!(!used(&[asked, user, read]));
        assert!(!used(&[user]));
        assert!(used(&[user, asked, "{\"role\":"]));
        let write = format!(
            r#"{{"role":"assistant","message":{{"content":[{{"type":"tool_use","name":"Write","input":{{"contents":"{}"}}}}]}}}}"#,
            "x".repeat(8 * 1024)
        );
        let long_turn = [user, asked]
            .into_iter()
            .chain(std::iter::repeat_n(write.as_str(), 256))
            .collect::<Vec<_>>();
        assert!(long_turn.join("\n").len() > 2 * 1024 * 1024);
        assert!(used(&long_turn), "a turn longer than any tail");

        let history = [user, asked].join("\n") + "\n";
        let cut = std::io::Read::chain(history.as_bytes(), Broken);
        assert!(
            super::used_question_box(std::io::BufReader::new(cut)).is_err(),
            "an interrupted scan is no answer"
        );
    }

    struct Broken;

    impl std::io::Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::ConnectionReset.into())
        }
    }

    use super::*;
    use crate::graph::PromptKey;
    use crate::screen::Session;

    fn gated(text: Option<&str>) -> Value {
        serde_json::from_str(&with_gate(text).unwrap().expect("the file changes")).unwrap()
    }

    #[test]
    fn a_missing_hooks_file_gets_every_no_op_entry() {
        let config = gated(None);
        assert_eq!(config["version"], 1);
        assert_eq!(config["hooks"]["stop"][0]["command"], "printf '{}\\n'");
        assert_eq!(
            config["hooks"]["beforeSubmitPrompt"][0]["command"],
            "printf '{\"continue\":true}\\n'"
        );
        assert_eq!(
            config["hooks"]["afterAgentResponse"][0]["command"],
            "printf '{}\\n'"
        );
        assert_eq!(config["hooks"]["stop"][0]["timeout"], HOOK_TIMEOUT_SECS);
    }

    #[test]
    fn the_gate_adds_only_missing_events_and_keeps_every_other_entry() {
        let orca = r#"{"version":1,"hooks":{"stop":[{"command":"orca stop"}],"preToolUse":[{"command":"orca pre"}],"afterFileEdit":[]},"extra":true}"#;
        let config = gated(Some(orca));
        assert_eq!(config["hooks"]["stop"], json!([{ "command": "orca stop" }]));
        assert_eq!(
            config["hooks"]["preToolUse"],
            json!([{ "command": "orca pre" }])
        );
        assert_eq!(config["hooks"]["afterFileEdit"], json!([]));
        assert_eq!(config["extra"], true);
        assert_eq!(
            config["hooks"]["beforeSubmitPrompt"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn an_empty_event_list_counts_as_missing() {
        let config = gated(Some(
            r#"{"hooks":{"stop":[],"beforeSubmitPrompt":[{"command":"x"}]}}"#,
        ));
        assert_eq!(config["hooks"]["stop"].as_array().unwrap().len(), 1);
        assert_eq!(
            config["hooks"]["beforeSubmitPrompt"],
            json!([{ "command": "x" }])
        );
    }

    #[test]
    fn a_gated_file_is_not_written_again() {
        let once = with_gate(None).unwrap().unwrap();
        assert_eq!(with_gate(Some(&once)).unwrap(), None);
    }

    #[test]
    fn a_file_that_is_not_a_hooks_object_is_left_alone() {
        for text in ["{not json", "[]", r#"{"hooks":[]}"#] {
            assert!(with_gate(Some(text)).is_err(), "{text}");
        }
    }

    #[test]
    fn opening_the_gate_writes_the_file_and_keeps_other_entries() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(USER_HOOKS);
        open_gate(&path).unwrap();
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["hooks"]["stop"].as_array().unwrap().len(), 1);
        std::fs::write(&path, r#"{"hooks":{"stop":[{"command":"mine"}]}}"#).unwrap();
        open_gate(&path).unwrap();
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["hooks"]["stop"], json!([{ "command": "mine" }]));
        assert_eq!(
            written["hooks"]["beforeSubmitPrompt"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        std::fs::write(&path, "{broken").unwrap();
        assert!(open_gate(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{broken");
        assert_eq!(
            std::fs::read_dir(home.path().join(".cursor"))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn concurrent_launches_each_read_a_whole_gated_file() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(USER_HOOKS);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mine = "x".repeat(16 * 1024);
        let start = json!({ "hooks": { "afterFileEdit": [{ "command": mine }] } }).to_string();
        for _ in 0..50 {
            std::fs::write(&path, &start).unwrap();
            let barrier = std::sync::Barrier::new(8);
            std::thread::scope(|s| {
                for _ in 0..8 {
                    s.spawn(|| {
                        barrier.wait();
                        open_gate(&path).unwrap();
                        let read: Value =
                            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
                        assert_eq!(read["hooks"]["afterFileEdit"][0]["command"], mine);
                        assert_eq!(read["hooks"]["stop"].as_array().unwrap().len(), 1);
                    });
                }
            });
        }
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_hooks_file_keeps_its_link_and_the_target_changes() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("dotfiles.json");
        std::fs::write(&target, r#"{"hooks":{"stop":[{"command":"mine"}]}}"#).unwrap();
        let path = home.path().join(USER_HOOKS);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        open_gate(&path).unwrap();
        assert!(
            std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(
            written["hooks"]["beforeSubmitPrompt"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&target, "{}").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        open_gate(&path).unwrap();
        let mode = std::fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn the_trust_box_has_buttons() {
        let prompt =
            crate::screen::prompt(&PROMPTS, include_str!("screens/cursor-trust.txt"), 100).unwrap();
        assert_eq!(
            prompt.body,
            ["Do you trust the contents of this directory?", "/tmp/repo"]
        );
        let options: Vec<_> = prompt
            .options
            .iter()
            .map(|o| (o.label.as_str(), &o.key))
            .collect();
        assert_eq!(
            options,
            [
                ("Trust this workspace", &PromptKey::Char('a')),
                ("Quit", &PromptKey::Char('q'))
            ]
        );
        assert_eq!(crate::screen::prompt(&PROMPTS, QUESTION, 80), None);
    }

    #[test]
    fn the_trust_box_is_a_trust_wait() {
        let screen = "│  Do you trust the contents of this directory?  │\n│  ▶ [a] Trust this workspace  │\n│    [q] Quit  │";
        assert_eq!(
            crate::screen::find(&DIALOGS, screen, Session::NotStarted).map(|(kind, _)| kind),
            Some(WaitKind::Trust)
        );
        assert_eq!(
            crate::screen::find(&DIALOGS, screen, Session::Started),
            None
        );
    }

    const QUESTION: &str = "  Use your ask-question tool to ask me one multiple-choice question: red or blue.\n\n  I'll ask that now.\n\n ┌──────────────────┐\n │ Clarifying Questions │\n │ Question 1 of 1 │\n │ 1. Red or blue? │\n │   › [ ] Red │\n │     [ ] Blue │\n │ ↑/↓ option · ←/→ question · Space select · Enter next/submit · Esc to skip │\n └──────────────────┘\n";
    const APPROVAL: &str = "  I'll create the directory and stop there.\n\n  $ mkdir zz_dir Waiting for approval...\n\n──────────────────────\n $  mkdir zz_dir in .\n\n Run this command?\n Not in allowlist: mkdir\n  → Run (once) (y)\n    Add Shell(mkdir) to allowlist? (tab)\n    Run Everything (shift+tab)\n    Skip & tell the agent what to do instead (esc or n)\n\n\n";
    fn idle_after(answer: &str) -> String {
        format!(
            "  $ touch y.txt 558ms\n\n  {answer}\n\n\n  → Add a follow-up\n\n\n  Claude Sonnet 5 300K High  [----------] 7%  cur @ master\n"
        )
    }

    const GUARDED: &str = concat!(
        "  $ mkdir zz_cursor_dir 0ms\n",
        "\n",
        "────────────────────────────────────────────────────────────────────────────────\n",
        " $  loom guard\n",
        "    mkdir zz_cursor_dir in .\n",
        "\n",
        " Run this command?\n",
        " Shell allowlist is empty\n",
        "  → Run (once) (y)\n",
        "    Add Shell(loom), Shell(mkdir) to allowlist? (tab)\n",
        "    Run Everything (shift+tab)\n",
        "    Skip & tell the agent what to do instead (esc or n)\n",
    );

    #[test]
    fn the_guard_line_is_not_the_command() {
        let prompt = crate::screen::prompt(&PROMPTS, GUARDED, 80).unwrap();
        assert_eq!(
            prompt.body,
            [
                "mkdir zz_cursor_dir in .",
                "Run this command?",
                "Shell allowlist is empty"
            ]
        );
        assert_eq!(prompt.summary, "mkdir zz_cursor_dir in .");
        assert_eq!(prompt.options.len(), 4);
    }

    #[test]
    fn approval_prompt_keys_come_from_the_hints() {
        let prompt = crate::screen::prompt(&PROMPTS, APPROVAL, 80).unwrap();
        assert_eq!(
            prompt.body,
            [
                "$  mkdir zz_dir in .",
                "Run this command?",
                "Not in allowlist: mkdir"
            ]
        );
        assert_eq!(prompt.summary, "$  mkdir zz_dir in .");
        let options: Vec<_> = prompt
            .options
            .iter()
            .map(|o| (o.label.as_str(), o.key.clone()))
            .collect();
        assert_eq!(
            options,
            [
                ("Run (once)", PromptKey::Char('y')),
                ("Add Shell(mkdir) to allowlist?", PromptKey::Tab),
                ("Run Everything", PromptKey::BackTab),
                (
                    "Skip & tell the agent what to do instead",
                    PromptKey::Char('n')
                ),
            ]
        );
        assert_eq!(crate::screen::prompt(&PROMPTS, QUESTION, 80), None);
    }

    #[test]
    fn question_and_approval_boxes_are_found_in_a_session() {
        assert_eq!(
            crate::screen::find(&DIALOGS, QUESTION, Session::Started).map(|(kind, _)| kind),
            Some(WaitKind::Question)
        );
        assert_eq!(
            crate::screen::find(&DIALOGS, APPROVAL, Session::Started).map(|(kind, _)| kind),
            Some(WaitKind::Approval)
        );
    }

    #[test]
    fn dialog_text_in_agent_output_is_not_a_box() {
        for text in [
            "Run this command?",
            "Enter next/submit",
            "Do you trust the contents of this directory?",
        ] {
            let middle = idle_after(&format!("{text}\n  Done."));
            assert_eq!(
                crate::screen::find(&DIALOGS, &middle, Session::Started),
                None,
                "{text}"
            );
            let last = idle_after(text);
            assert_eq!(
                crate::screen::find(&DIALOGS, &last, Session::Started),
                None,
                "{text}"
            );
        }
    }

    const PRINTED: &str = "  Reply with exactly this text.\n\n\n  ┌───┬───┐\n  │ A │ B │\n  ├───┼───┤\n  │ 1 │ 2 │\n  └───┴───┘\n\n  ─────\n  Run this command?\n  Enter next/submit\n  done\n\n\n\n\n  → Add a follow-up\n\n\n  Auto Balance Balance  [----------] 5%  cur @ master\n";
    const QUESTION_BELOW_TABLE: &str = "  ┌───┬───┐\n  │ A │ B │\n  ├───┼───┤\n  │ 1 │ 2 │\n  └───┴───┘\n\n ┌──────────────────┐\n │ Clarifying Questions │\n │ Question 1 of 1 │\n │ 1. Red or blue? │\n │   › [ ] red │\n │     [ ] blue │\n │ ↑/↓ option · ←/→ question · Space select · Enter next/submit · Esc to skip │\n └──────────────────┘\n";

    #[test]
    fn dialog_text_below_a_printed_rule_or_box_is_not_a_box() {
        let in_box = idle_after(
            "┌───────────────────┐\n  │ Run this command? │\n  │ Enter next/submit │\n  └───────────────────┘",
        );
        let working = PRINTED.replace("→ Add a follow-up", "⠠⠛ Working\n\n  → Add a follow-up");
        for screen in [PRINTED, &in_box, &working] {
            assert_eq!(
                crate::screen::find(&DIALOGS, screen, Session::Started),
                None,
                "{screen}"
            );
        }
        assert_eq!(
            crate::screen::find(&DIALOGS, QUESTION_BELOW_TABLE, Session::Started),
            Some((WaitKind::Question, "Enter next/submit".into()))
        );
    }

    fn event(input: Value) -> Event {
        serde_json::from_value(input).unwrap()
    }

    fn delivery(kind: Kind) -> Delivery {
        Delivery {
            id: crate::graph::MessageId(7),
            from: "총괄".into(),
            kind,
            body: "go".into(),
            what: Some(crate::protocol::MessageKind::Note),
        }
    }

    const TRANSCRIPT: &str = "/h/.cursor/projects/tmp-r/agent-transcripts/a1ab/a1ab.jsonl";

    fn tool_event(name: &str, tool: &str, input: Value, extra: Value) -> Event {
        let mut e = json!({
            "hook_event_name": name, "conversation_id": "a1ab", "session_id": "a1ab",
            "tool_name": tool, "tool_input": input, "tool_use_id": "toolu_01",
            "transcript_path": TRANSCRIPT,
        });
        e.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        event(e)
    }

    #[test]
    fn reads_measured_inputs() {
        let pre = tool_event(
            "preToolUse",
            "Shell",
            json!({ "command": "echo hi", "cwd": "", "timeout": 30000 }),
            json!({ "model": "gpt-5.3-codex" }),
        );
        assert_eq!(pre.take(), Some(Take::Now(&[Kind::Interrupt])));
        assert_eq!(pre.model(), None);
        let stop = event(json!({
            "hook_event_name": "stop", "status": "completed", "loop_count": 0, "model": "gpt-5.3-codex-low",
        }));
        assert_eq!(stop.take(), Some(Take::WhenIdle));
        assert_eq!(
            stop.model().and_then(|m| m.model).as_deref(),
            Some("gpt-5.3-codex-low")
        );
        let shell = tool_event(
            "postToolUse",
            "Shell",
            json!({ "command": "sleep 9", "cwd": "", "timeout": 20000 }),
            json!({ "duration": 9063.009, "tool_output": "{\"output\":\"\",\"exitCode\":0}" }),
        );
        assert_eq!(shell.take(), Some(Take::Now(Kind::ALL)));
        let background = tool_event(
            "postToolUse",
            "Shell",
            json!({ "command": "sleep 4", "cwd": "" }),
            json!({ "duration": 40.0 }),
        );
        assert_eq!(background.take(), Some(Take::Now(Kind::ALL)));
        for (tool, input) in [
            ("Read", json!({ "file_path": "/r/a.txt" })),
            ("Write", json!({ "file_path": "/r/b.txt" })),
            ("Grep", json!({ "pattern": "hello" })),
            ("Task", json!({ "description": "d", "prompt": "p" })),
        ] {
            let post = tool_event("postToolUse", tool, input, json!({ "duration": 20.0 }));
            assert_eq!(post.take(), Some(Take::Now(Kind::ALL)), "{tool}");
        }
        let failed = tool_event(
            "postToolUseFailure",
            "Shell",
            json!({ "command": "false", "cwd": "", "timeout": 30000 }),
            json!({ "error_message": "exit 1", "failure_type": "error", "is_interrupt": false }),
        );
        assert_eq!(failed.take(), Some(Take::Now(&[Kind::Interrupt])));
    }

    #[test]
    fn tool_calls_whose_hook_context_the_model_never_sees_take_nothing() {
        let poll = json!({ "file_path": "/h/.cursor/projects/tmp-r/terminals/618716.txt" });
        for name in ["preToolUse", "postToolUse", "postToolUseFailure"] {
            let read = tool_event(name, "Read", poll.clone(), json!({ "duration": 0.4 }));
            assert_eq!(read.take(), None, "AwaitShell poll {name}");
        }
        let moved = tool_event(
            "postToolUse",
            "Shell",
            json!({ "command": "sleep 8 && echo late", "cwd": "", "timeout": 2000 }),
            json!({ "duration": 2094.747, "tool_output": "{\"output\":\"\",\"exitCode\":0}" }),
        );
        assert_eq!(moved.take(), None, "a shell sent to the background");
        let mut subagent = json!({
            "hook_event_name": "postToolUse", "conversation_id": "81db", "session_id": "81db",
            "tool_name": "Shell", "tool_input": { "command": "echo sub", "cwd": "", "timeout": 30000 },
            "tool_use_id": "73ce", "duration": 54.6, "transcript_path": null,
        });
        assert_eq!(event(subagent.clone()).take(), None, "a subagent tool");
        subagent["hook_event_name"] = "preToolUse".into();
        assert_eq!(event(subagent).take(), None, "a subagent tool");
        let mcp = tool_event(
            "postToolUse",
            "MCP:search",
            json!({}),
            json!({ "duration": 10.0 }),
        );
        assert_eq!(mcp.take(), None, "a tool not measured");
    }

    #[test]
    fn idle_mail_becomes_a_followup_and_interrupt_denies_the_tool() {
        let stop = event(json!({ "hook_event_name": "stop" }));
        let reply = stop.reply(&[delivery(Kind::Queue)], None);
        assert!(reply["followup_message"].as_str().unwrap().contains("#7"));
        let pre = tool_event("preToolUse", "Read", json!({}), json!({}));
        let denied = pre.reply(&[delivery(Kind::Interrupt)], None);
        assert_eq!(denied["permission"], "deny");
        assert!(
            denied["additional_context"]
                .as_str()
                .unwrap()
                .contains("#7")
        );
        assert_eq!(denied.get("agent_message"), None);
    }

    #[test]
    fn foreground_shell_gets_the_guard_line() {
        let pre = event(json!({
            "hook_event_name": "preToolUse", "tool_name": "Shell",
            "tool_input": { "command": "npm test", "timeout": 30000 },
        }));
        assert_eq!(
            pre.reply(&[], None)["updated_input"],
            json!({ "command": "loom guard\nnpm test", "timeout": 30000 })
        );
        let background = event(json!({
            "hook_event_name": "preToolUse", "tool_name": "Shell",
            "tool_input": { "command": "loom wait", "is_background": true },
        }));
        assert_eq!(background.reply(&[], None), json!({}));
    }

    #[test]
    fn after_agent_response_carries_the_reply_text() {
        let said = event(json!({ "hook_event_name": "afterAgentResponse", "text": "병합할까요?" }));
        assert!(matches!(&said, Event::AfterAgentResponse { text } if text == "병합할까요?"));
        assert_eq!(said.take(), None);
        assert_eq!(said.reply(&[], None), json!({}));
        assert!(hooks()["hooks"]["afterAgentResponse"].is_array());
    }

    #[test]
    fn session_start_contexts_have_the_ask_rules() {
        super::super::assert_every_launch_asks(|launch| {
            let rules = rules(launch)["additional_context"]
                .as_str()
                .unwrap()
                .to_owned();
            (usage(), rules)
        });
    }

    #[test]
    fn each_session_start_context_fits() {
        assert!(usage().chars().count() <= CONTEXT_LIMIT_CHARS);
        for position in [Position::Root, Position::Child] {
            let launch = Launch {
                position,
                skills: AlwaysOn::default(),
            };
            let text = rules(launch)["additional_context"]
                .as_str()
                .unwrap()
                .to_owned();
            assert!(text.chars().count() <= CONTEXT_LIMIT_CHARS, "{position:?}");
        }
    }

    #[test]
    fn skills_env_round_trips_and_defaults_to_all_on() {
        assert_eq!(skills_from(None), AlwaysOn::default());
        assert_eq!(skills_from(Some("x")), AlwaysOn::default());
        let off = AlwaysOn {
            o_mode: false,
            ..AlwaysOn::default()
        };
        assert_eq!(skills_from(Some(&skills_value(off))), off);
    }

    #[test]
    fn model_and_effort_join_into_one_id() {
        let choice = |model: Option<&str>, effort: Option<&str>| super::super::ModelChoice {
            model: model.map(Into::into),
            effort: effort.map(Into::into),
        };
        assert_eq!(
            choice_args(&choice(Some("claude-opus-5-5"), Some("high"))).unwrap(),
            ["--model", "claude-opus-5-5-high"]
        );
        assert_eq!(choice_args(&choice(None, Some("high"))), None);
    }
}
