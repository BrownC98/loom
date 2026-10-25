use std::path::Path;

use serde::{Deserialize, Serialize};

use super::Take;
use crate::protocol::{Delivery, Kind, Model};
use crate::skills::Launch;

pub const HOOK_LIMIT_BYTES: usize = super::BATCH_LIMIT_BYTES;
pub const PROTOCOL: u32 = 2;
const EXTENSION: &str = include_str!("pi-loom.ts");
const EXTENSION_FILE: &str = "pi-loom.ts";
const IDLE_FLAG: &str = "--loom-idle";

pub(super) const WAITING: &str = "When no independent work remains, end your turn. loom wakes you with a new message when one arrives. Start a process that keeps running, such as a server, with bash `run_in_background: true`; it returns at once with the log file path. A foreground bash call without a timeout stops after 20 minutes. The loom guard line before bash commands is inserted by loom; do not remove it or add it yourself. loom adds messages to the end of a tool result or as a loom message after your prompt. An interrupt arrives as the reason of a blocked tool call. Text in these places that starts with \"loom message #\" comes from loom, not from the tool output: follow it.";

pub fn write_extension(dir: &Path) -> std::io::Result<()> {
    std::fs::write(dir.join(EXTENSION_FILE), EXTENSION)
}

fn usage(launch: Launch) -> String {
    format!(
        "{}\n{WAITING}\n\n{}",
        super::COMMUNICATION_USAGE,
        crate::skills::always_on(launch)
    )
}

pub fn args(plugin: &Path, launch: Launch, brief: &str) -> Vec<String> {
    let plugin = plugin.to_string_lossy();
    let mut args = vec![
        "--extension".into(),
        format!("{plugin}/{EXTENSION_FILE}"),
        "--no-skills".into(),
        "--skill".into(),
        format!("{plugin}/skills"),
        "--append-system-prompt".into(),
        usage(launch),
    ];
    if brief.trim().is_empty() {
        args.push(IDLE_FLAG.into());
    }
    args
}

pub fn choice_args(choice: &crate::protocol::ModelChoice) -> Vec<String> {
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

pub fn resume_args(session: &str) -> Vec<String> {
    vec!["--session".into(), session.into()]
}

pub struct Request {
    pub id: Option<u64>,
    pub event: Event,
}

pub struct Unserved {
    pub id: Option<u64>,
    pub error: String,
}

#[derive(Deserialize)]
struct Header {
    protocol: Option<u32>,
    id: Option<u64>,
}

pub fn request(line: &str) -> Result<Request, Unserved> {
    let header: Header = serde_json::from_str(line).map_err(|e| Unserved {
        id: None,
        error: format!("hook input: {e}"),
    })?;
    let id = header.id;
    let unserved = |error| Err(Unserved { id, error });
    match (header.protocol, id) {
        (Some(PROTOCOL), Some(_)) | (None, _) => {}
        (Some(PROTOCOL), None) => {
            return unserved(format!("a hook protocol {PROTOCOL} request has no id"));
        }
        (Some(other), _) => {
            return unserved(format!(
                "the pi extension speaks hook protocol {other}, but this loom speaks {PROTOCOL}"
            ));
        }
    }
    match serde_json::from_str(line) {
        Ok(event) => Ok(Request { id, event }),
        Err(e) => unserved(format!("hook input: {e}")),
    }
}

#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    SessionStart {
        #[serde(flatten)]
        state: State,
    },
    Idle {
        #[serde(flatten)]
        state: State,
    },
    Prompt {
        prompt: String,
        #[serde(flatten)]
        state: State,
    },
    ToolCall {
        #[serde(flatten)]
        state: State,
    },
    ToolResult {
        #[serde(flatten)]
        state: State,
    },
    Settled {
        reply: Option<String>,
        #[serde(flatten)]
        state: State,
    },
}

#[derive(Deserialize)]
pub struct State {
    transcript: Option<String>,
    model: Option<String>,
    effort: Option<String>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Reply {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl Event {
    fn state(&self) -> &State {
        match self {
            Event::SessionStart { state }
            | Event::Idle { state }
            | Event::Prompt { state, .. }
            | Event::ToolCall { state }
            | Event::ToolResult { state }
            | Event::Settled { state, .. } => state,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Event::SessionStart { .. } => "session_start",
            Event::Idle { .. } => "idle",
            Event::Prompt { .. } => "prompt",
            Event::ToolCall { .. } => "tool_call",
            Event::ToolResult { .. } => "tool_result",
            Event::Settled { .. } => "settled",
        }
    }

    pub fn take(&self) -> Option<Take> {
        Some(match self {
            Event::SessionStart { .. } => return None,
            Event::Idle { .. } | Event::Settled { .. } => Take::WhenIdle,
            Event::Prompt { .. } | Event::ToolResult { .. } => Take::Now(Kind::ALL),
            Event::ToolCall { .. } => Take::Now(&[Kind::Interrupt]),
        })
    }

    pub fn transcript(&self) -> Option<String> {
        self.state().transcript.clone()
    }

    pub fn model(&self) -> Option<Model> {
        match self {
            Event::SessionStart { state }
            | Event::Prompt { state, .. }
            | Event::Settled { state, .. }
                if state.model.is_some() =>
            {
                Some(Model {
                    model: state.model.clone(),
                    effort: state.effort.clone(),
                })
            }
            _ => None,
        }
    }

    pub fn prompt(&self) -> Option<&str> {
        match self {
            Event::Prompt { prompt, .. } => Some(prompt),
            _ => None,
        }
    }

    pub fn turn_ends(&self) -> Option<Option<String>> {
        match self {
            Event::Settled { reply, .. } => Some(reply.clone()),
            _ => None,
        }
    }
}

pub fn reply(id: Option<u64>, taken: &[Delivery], note: Option<&str>) -> Reply {
    Reply {
        id,
        text: super::hook_text(taken, note),
        error: None,
    }
}

pub fn nudge(id: Option<u64>, text: String) -> Reply {
    Reply {
        id,
        text: Some(text),
        error: None,
    }
}

pub fn failed(id: Option<u64>, error: String) -> Reply {
    Reply {
        id,
        text: None,
        error: Some(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{AlwaysOn, Position};

    fn event(json: &str) -> Event {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn every_extension_event_reaches_a_take() {
        assert_eq!(event(r#"{"event":"session_start"}"#).take(), None);
        assert_eq!(
            event(r#"{"event":"idle","transcript":"/s.jsonl"}"#).take(),
            Some(Take::WhenIdle)
        );
        assert_eq!(
            event(r#"{"event":"settled","reply":"ok"}"#).take(),
            Some(Take::WhenIdle)
        );
        assert_eq!(
            event(r#"{"event":"prompt","prompt":"hi"}"#).take(),
            Some(Take::Now(Kind::ALL))
        );
        assert_eq!(
            event(r#"{"event":"tool_result"}"#).take(),
            Some(Take::Now(Kind::ALL))
        );
        assert_eq!(
            event(r#"{"event":"tool_call"}"#).take(),
            Some(Take::Now(&[Kind::Interrupt]))
        );
    }

    #[test]
    fn the_model_is_the_provider_and_id_with_the_thinking_level() {
        let start = event(
            r#"{"event":"session_start","transcript":"/s/a_b.jsonl","model":"kta-hub/kta-chat-vl-pro","effort":"max"}"#,
        );
        assert_eq!(
            start.model(),
            Some(Model {
                model: Some("kta-hub/kta-chat-vl-pro".into()),
                effort: Some("max".into()),
            })
        );
        assert_eq!(start.transcript().as_deref(), Some("/s/a_b.jsonl"));
        assert_eq!(event(r#"{"event":"settled"}"#).model(), None);
        assert_eq!(event(r#"{"event":"tool_call","model":"m"}"#).model(), None);
    }

    #[test]
    fn only_a_settled_turn_reports_its_reply() {
        assert_eq!(
            event(r#"{"event":"settled","reply":"done"}"#).turn_ends(),
            Some(Some("done".into()))
        );
        assert_eq!(event(r#"{"event":"idle"}"#).turn_ends(), None);
        assert_eq!(
            event(r#"{"event":"prompt","prompt":"p"}"#).prompt(),
            Some("p")
        );
    }

    #[test]
    fn an_empty_reply_has_no_text() {
        assert_eq!(
            serde_json::to_string(&reply(Some(3), &[], None)).unwrap(),
            r#"{"id":3}"#
        );
        assert_eq!(
            serde_json::to_string(&reply(Some(4), &[], Some("n"))).unwrap(),
            r#"{"id":4,"text":"n"}"#
        );
        assert_eq!(
            serde_json::to_string(&failed(Some(5), "down".into())).unwrap(),
            r#"{"id":5,"error":"down"}"#
        );
        let request = served(r#"{"protocol":2,"id":7,"event":"settled","reply":"r","model":"m"}"#);
        assert_eq!(request.id, Some(7));
        assert_eq!(request.event.turn_ends(), Some(Some("r".into())));
    }

    fn served(line: &str) -> Request {
        request(line).unwrap_or_else(|u| panic!("{line}: {}", u.error))
    }

    fn unserved(line: &str) -> Unserved {
        match request(line) {
            Ok(_) => panic!("{line} is served"),
            Err(unserved) => unserved,
        }
    }

    #[test]
    fn a_request_without_id_or_protocol_is_the_old_one_shot_form() {
        let old = served(r#"{"event":"tool_result","transcript":"/s.jsonl"}"#);
        assert_eq!(old.id, None);
        assert_eq!(old.event.take(), Some(Take::Now(Kind::ALL)));
        assert_eq!(
            serde_json::to_string(&reply(old.id, &[], Some("n"))).unwrap(),
            r#"{"text":"n"}"#
        );
        let unversioned = served(r#"{"id":4,"event":"idle"}"#);
        assert_eq!(unversioned.id, Some(4));
    }

    #[test]
    fn an_unknown_protocol_or_event_is_refused_with_the_id() {
        let newer = unserved(r#"{"protocol":3,"id":9,"event":"idle"}"#);
        assert_eq!(newer.id, Some(9));
        assert!(newer.error.contains("protocol 3"), "{}", newer.error);
        assert!(
            newer.error.contains(&format!("speaks {PROTOCOL}")),
            "{}",
            newer.error
        );
        let event = unserved(r#"{"protocol":2,"id":5,"event":"compacted"}"#);
        assert_eq!(event.id, Some(5));
        assert!(event.error.contains("compacted"), "{}", event.error);
        assert_eq!(unserved(r#"{"protocol":2,"event":"idle"}"#).id, None);
        assert_eq!(unserved("not json").id, None);
    }

    #[test]
    fn every_event_the_extension_sends_is_served() {
        let after = |mark| EXTENSION.split(mark).skip(1);
        let sent: Vec<&str> = after("event: \"")
            .chain(after("wait(\""))
            .filter_map(|rest| rest.split('"').next())
            .collect();
        for event in [
            "session_start",
            "idle",
            "prompt",
            "tool_call",
            "tool_result",
            "settled",
        ] {
            assert!(
                sent.contains(&event),
                "{event} is not in the extension: {sent:?}"
            );
        }
        for event in sent {
            let line =
                format!(r#"{{"protocol":{PROTOCOL},"id":1,"event":"{event}","prompt":"p"}}"#);
            assert_eq!(served(&line).event.name(), event);
        }
    }

    #[test]
    fn a_launch_without_a_brief_waits_for_mail() {
        let launch = Launch {
            position: Position::Child,
            skills: AlwaysOn::default(),
        };
        let plugin = Path::new("/p/plugin");
        let args = super::args(plugin, launch, "  ");
        assert_eq!(
            args[..5],
            [
                "--extension",
                "/p/plugin/pi-loom.ts",
                "--no-skills",
                "--skill",
                "/p/plugin/skills"
            ]
        );
        assert_eq!(args[5], "--append-system-prompt");
        assert!(args[6].contains(super::super::COMMUNICATION_USAGE));
        assert!(args[6].contains(&crate::skills::always_on(launch)));
        assert!(args[6].contains("run_in_background: true"));
        assert_eq!(args.last().unwrap(), IDLE_FLAG);
        assert!(!super::args(plugin, launch, "Goal: x").contains(&IDLE_FLAG.to_owned()));
    }

    #[test]
    fn every_launch_asks_the_human_through_loom() {
        super::super::assert_every_launch_asks(|launch| {
            let text = usage(launch);
            (text.clone(), text)
        });
    }

    #[test]
    fn the_extension_names_the_hook_verb_and_the_idle_flag() {
        assert!(EXTENSION.contains(r#"spawn("loom", ["pi-hook"]"#));
        assert!(EXTENSION.contains(r#"pi.on("resources_discover""#));
        assert!(EXTENSION.contains(&format!(r#"IDLE_FLAG = "{}""#, &IDLE_FLAG[2..])));
        assert!(EXTENSION.contains(&format!("PROTOCOL = {PROTOCOL};")));
        assert!(EXTENSION.contains("{ protocol: PROTOCOL, id, ...input }"));
        let dir = tempfile::tempdir().unwrap();
        write_extension(dir.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join(EXTENSION_FILE)).unwrap(),
            EXTENSION
        );
    }

    #[cfg(unix)]
    fn run_extension(fake_loom: &str, script: &str) -> (String, String) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        write_extension(dir.path()).unwrap();
        let module = |name: &str, code: &str| {
            let at = dir.path().join("node_modules").join(name);
            std::fs::create_dir_all(&at).unwrap();
            std::fs::write(
                at.join("package.json"),
                r#"{"type":"module","main":"index.js"}"#,
            )
            .unwrap();
            std::fs::write(at.join("index.js"), code).unwrap();
        };
        module(
            "@earendil-works/pi-coding-agent",
            "export class DefaultPackageManager {};
             export class SettingsManager {
               static create(cwd, dir, options) {
                 return { getShellCommandPrefix: () => (options?.projectTrusted ? 'PREFIX=set' : 'PREFIX=untrusted'), getShellPath: () => '/bin/sh' };
               }
             }
             export const getShellConfig = (path) => ({ shell: path, args: ['-c'] });
             export const getAgentDir = () => ''; export const loadSkills = () => ({ skills: [] });
             export const createBashToolDefinition = (cwd, options) => ({
               name: 'bash', description: 'Run bash.', parameters: { properties: { command: {}, timeout: {} } },
               execute: async (id, params) => {
                 if (params.command === 'hang') throw new Error('Command timed out after ' + params.timeout + ' seconds');
                 return { content: [{ type: 'text', text: JSON.stringify({ ...params, ...options }) }] };
               },
             });",
        );
        module(
            "typebox",
            "export const Type = { Object: (properties) => ({ type: 'object', properties }), Optional: (x) => x, Boolean: (o) => ({ type: 'boolean', ...o }) };",
        );
        std::fs::write(dir.path().join("package.json"), r#"{"type":"module"}"#).unwrap();
        let loom = dir.path().join("bin/loom");
        std::fs::create_dir_all(loom.parent().unwrap()).unwrap();
        let body = format!(
            "#!/bin/sh\necho start >> \"$0.in\"\nstart=$(grep -c start \"$0.in\")\n\
             answer() {{ id=$(echo \"$1\" | sed 's/.*\"id\":\\([0-9]*\\).*/\\1/'); echo \"{{\\\"id\\\":$id$2}}\"; }}\n{fake_loom}\n"
        );
        std::fs::write(&loom, body).unwrap();
        std::fs::set_permissions(&loom, std::fs::Permissions::from_mode(0o755)).unwrap();
        let harness = format!(
            "import ext from './{EXTENSION_FILE}';
             const on = {{}}; const sent = []; const tools = {{}};
             ext({{ registerFlag() {{}}, getFlag: () => false, on: (n, f) => (on[n] = f), sendMessage: (m) => sent.push(m), registerTool: (t) => (tools[t.name] = t) }});
             const ctx = {{ isIdle: () => true, isProjectTrusted: () => true, hasUI: false, cwd: process.cwd(), sessionManager: {{ getSessionFile: () => '/s.jsonl' }}, ui: {{ notify() {{}} }} }};
             const until = async (done, ms) => {{ for (let t = 0; t < ms && !done(); t += 50) await new Promise((r) => setTimeout(r, 50)); }};
             {script}
             on.session_shutdown();"
        );
        std::fs::write(dir.path().join("harness.mjs"), harness).unwrap();
        let path = format!(
            "{}:{}",
            loom.parent().unwrap().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let out = std::process::Command::new("node")
            .arg("harness.mjs")
            .current_dir(dir.path())
            .env("PATH", path)
            .env("LOOM_TASK_DIR", dir.path())
            .output()
            .expect("node runs the pi extension");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let calls = std::fs::read_to_string(loom.with_extension("in")).unwrap_or_default();
        (String::from_utf8_lossy(&out.stdout).into_owned(), calls)
    }

    #[cfg(unix)]
    const LOG_LINES: &str = "while read -r line; do echo \"$line\" >> \"$0.in\"; ";

    #[cfg(unix)]
    #[test]
    fn one_hook_process_answers_every_event() {
        let (out, calls) = run_extension(
            &format!("{LOG_LINES}answer \"$line\"; done"),
            "for (let i = 0; i < 3; i++) console.log(JSON.stringify(await on.tool_call({ toolName: 'read', input: {} }, ctx)));",
        );
        assert_eq!(calls.matches("start").count(), 1, "{calls}");
        assert_eq!(
            calls.matches(r#""event":"tool_call""#).count(),
            3,
            "{calls}"
        );
        assert_eq!(out.lines().collect::<Vec<_>>(), ["undefined"; 3]);
    }

    #[cfg(unix)]
    #[test]
    fn an_idle_wait_survives_the_death_of_its_hook_process() {
        let fake = format!(
            "[ \"$start\" = 1 ] && {{ echo 'WSL ERROR: UtilAcceptVsock' >&2; exit 1; }}\n\
             {LOG_LINES}[ \"$start\" = 2 ] && exit 1; answer \"$line\" ',\"text\":\"loom message #1\"'; done"
        );
        let (out, calls) = run_extension(
            &fake,
            "on.agent_settled({ aborted: false }, ctx);
             await until(() => sent.length, 8000);
             console.log(JSON.stringify(sent));",
        );
        assert!(out.contains("loom message #1"), "{out}");
        assert_eq!(calls.matches("start").count(), 3, "{calls}");
        assert_eq!(calls.matches(r#""event":"settled""#).count(), 2, "{calls}");
    }

    #[cfg(unix)]
    #[test]
    fn an_answered_or_busy_idle_wait_is_not_opened_again() {
        let (_, calls) = run_extension(
            &format!("{LOG_LINES}answer \"$line\"; done"),
            "on.agent_settled({ aborted: false }, ctx);
             await new Promise((r) => setTimeout(r, 2500));",
        );
        assert_eq!(calls.matches("settled").count(), 1, "{calls}");
        let (_, calls) = run_extension(
            &format!("{LOG_LINES}exit 1; done"),
            "ctx.isIdle = () => false;
             on.agent_settled({ aborted: false }, ctx);
             await new Promise((r) => setTimeout(r, 2500));",
        );
        assert_eq!(calls.matches("settled").count(), 1, "{calls}");
    }

    #[cfg(unix)]
    #[test]
    fn bash_starts_a_background_process_and_bounds_a_foreground_one() {
        let (out, _) = run_extension(
            &format!("{LOG_LINES}answer \"$line\"; done"),
            "const bash = tools.bash;
             const input = { command: 'echo up $PREFIX; sleep 30', run_in_background: true };
             await on.tool_call({ toolName: 'bash', input }, ctx);
             const t = Date.now();
             const started = (await bash.execute('1', input, undefined, undefined, ctx)).content[0].text;
             const pid = Number(started.match(/pid (\\d+)/)[1]);
             const log = started.match(/output in (\\S+)\\./)[1];
             const fs = await import('node:fs');
             await until(() => fs.readFileSync(log, 'utf8').includes('set'), 3000);
             console.log(JSON.stringify({ fast: Date.now() - t < 1000, guard: input.command.startsWith('loom guard'), log: fs.readFileSync(log, 'utf8'), alive: process.kill(pid, 0) }));
             process.kill(-pid);
             console.log((await bash.execute('2', { command: 'ls' }, undefined, undefined, ctx)).content[0].text);
             const untrusted = { ...ctx, isProjectTrusted: () => false };
             console.log(JSON.parse((await bash.execute('5', { command: 'ls' }, undefined, undefined, untrusted)).content[0].text).commandPrefix);
             console.log(await bash.execute('3', { command: 'hang' }, undefined, undefined, ctx).catch((e) => e.message));
             console.log('run_in_background' in bash.parameters.properties);
             const two = await Promise.all([1, 2].map((n) => bash.execute(String(n), { command: 'true', run_in_background: true }, undefined, undefined, ctx)));
             const logs = two.map((r) => r.content[0].text.match(/output in (\\S+)\\./)[1]);
             console.log(logs[0] !== logs[1]);
             const gone = { ...ctx, cwd: '/nonexistent' };
             console.log(await bash.execute('4', { command: 'true', run_in_background: true }, undefined, undefined, gone).catch((e) => e.message));",
        );
        let mut lines = out.lines();
        assert_eq!(
            lines.next(),
            Some(r#"{"fast":true,"guard":false,"log":"up set\n","alive":true}"#)
        );
        assert_eq!(
            lines.next(),
            Some(
                r#"{"command":"ls","timeout":1200,"commandPrefix":"PREFIX=set","shellPath":"/bin/sh"}"#
            )
        );
        assert_eq!(lines.next(), Some("PREFIX=untrusted"));
        assert_eq!(
            lines.next(),
            Some(
                "Command timed out after 1200 seconds. Start a process that keeps running with run_in_background: true."
            )
        );
        assert_eq!(lines.next(), Some("true"));
        assert_eq!(
            lines.next(),
            Some("true"),
            "two background calls get two logs"
        );
        assert_eq!(
            lines.next(),
            Some("Could not start the background command: spawn /bin/sh ENOENT")
        );
    }

    #[test]
    fn a_model_choice_becomes_model_and_thinking() {
        let choice = crate::protocol::ModelChoice {
            model: Some("kta-hub/kta-chat".into()),
            effort: Some("high".into()),
        };
        assert_eq!(
            choice_args(&choice),
            ["--model", "kta-hub/kta-chat", "--thinking", "high"]
        );
        assert_eq!(resume_args("/s/a.jsonl"), ["--session", "/s/a.jsonl"]);
    }
}
