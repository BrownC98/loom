use std::path::Path;

use crate::skills::Launch;

const EXTENSION: &str = include_str!("omp-loom.ts");
const EXTENSION_FILE: &str = "omp-loom.ts";

pub(super) const WAITING: &str = "When no independent work remains, end your turn. loom wakes you with a new message when one arrives. Start a process that keeps running, such as a server, with bash `async: true, timeout: 0`; it returns at once with a job ID. A bash call without a timeout stops after 5 minutes. The loom guard line before bash commands is inserted by loom; do not remove it or add it yourself. loom adds messages to the end of a tool result or as a loom message after your prompt. An interrupt arrives as the reason of a blocked tool call. Text in these places that starts with \"loom message #\" comes from loom, not from the tool output: follow it.";

pub struct Config {
    pub file: String,
    text: String,
}

pub fn config(plugin: &Path, launch: Launch) -> Config {
    let ignored: Vec<String> = crate::skills::names(launch.skills)
        .flat_map(|name| [format!("*/{name}"), format!("*/{name}~*")])
        .collect();
    let text = serde_json::json!({
        "skills": {
            "customDirectories": [format!("{}/skills", plugin.to_string_lossy())],
            "ignoredSkills": ignored,
        }
    })
    .to_string();
    let file = format!("omp-{}.yml", &crate::skills::digest(&[&text])[..16]);
    Config { file, text }
}

pub fn write_extension(dir: &Path) -> std::io::Result<()> {
    std::fs::write(dir.join(EXTENSION_FILE), EXTENSION)
}

pub fn write_config(dir: &Path, config: &Config) -> std::io::Result<()> {
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

pub fn args(plugin: &Path, launch: Launch, brief: &str) -> Vec<String> {
    let at = plugin.to_string_lossy();
    let mut args = vec![
        "--extension".into(),
        format!("{at}/{EXTENSION_FILE}"),
        "--config".into(),
        format!("{at}/{}", config(plugin, launch).file),
        "--append-system-prompt".into(),
        super::pi::usage(WAITING, launch),
    ];
    if brief.trim().is_empty() {
        args.push(super::pi::IDLE_FLAG.into());
    }
    args
}

pub fn resume_args(session: &str) -> Vec<String> {
    vec!["--resume".into(), session.into()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{AlwaysOn, Position};

    fn launch(skills: AlwaysOn) -> Launch {
        Launch {
            position: Position::Child,
            skills,
        }
    }

    #[test]
    fn a_launch_loads_the_extension_and_the_skill_config() {
        let plugin = Path::new("/p/plugin");
        let on = launch(AlwaysOn::default());
        let args = args(plugin, on, " ");
        assert_eq!(args[..2], ["--extension", "/p/plugin/omp-loom.ts"]);
        assert_eq!(args[2], "--config");
        assert_eq!(args[3], format!("/p/plugin/{}", config(plugin, on).file));
        assert_eq!(args[4], "--append-system-prompt");
        assert!(args[5].contains(super::super::COMMUNICATION_USAGE));
        assert!(args[5].contains("async: true"));
        assert_eq!(args.last().unwrap(), super::super::pi::IDLE_FLAG);
        assert!(
            !super::args(plugin, on, "Goal: x").contains(&super::super::pi::IDLE_FLAG.to_owned())
        );
        assert_eq!(resume_args("/s/a.jsonl"), ["--resume", "/s/a.jsonl"]);
    }

    #[test]
    fn the_config_adds_the_loom_skills_and_hides_other_copies_of_them() {
        let off = AlwaysOn {
            o_mode: false,
            ..AlwaysOn::default()
        };
        let config = super::config(Path::new("/p/plugin-off/o-mode"), launch(off));
        let value: serde_json::Value = serde_json::from_str(&config.text).unwrap();
        assert_eq!(
            value["skills"]["customDirectories"],
            serde_json::json!(["/p/plugin-off/o-mode/skills"])
        );
        let ignored = value["skills"]["ignoredSkills"].as_array().unwrap();
        assert!(ignored.contains(&"*/user-doc".into()));
        assert!(ignored.contains(&"*/ask-human~*".into()));
        assert!(!ignored.contains(&"*/o-mode".into()));
        assert_ne!(
            config.file,
            super::config(Path::new("/p/plugin"), launch(AlwaysOn::default())).file
        );
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path(), &config).unwrap();
        write_config(dir.path(), &config).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join(&config.file)).unwrap(),
            config.text
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn the_extension_serves_only_the_main_session_and_guards_foreground_bash() {
        use super::super::pi::tests::{LOG_LINES, run};
        let (out, calls) = run(
            EXTENSION_FILE,
            &format!("{LOG_LINES}answer \"$line\"; done"),
            "const sub = { ...ctx, agent: { kind: 'sub' } };
             await on.session_start({}, sub);
             await on.session_start({}, ctx);
             console.log(JSON.stringify(await on.tool_call({ toolName: 'bash', input: { command: 'ls' } }, ctx)));
             console.log(JSON.stringify(await on.tool_call({ toolName: 'bash', input: { command: 'srv', async: true } }, ctx)));
             console.log(JSON.stringify(await on.tool_call({ toolName: 'bash', input: { command: 'x' } }, sub)));
             console.log(JSON.stringify(await on.tool_call({ toolName: 'bash', input: { command: 'loom guard\\nls' } }, ctx)));",
        );
        assert_eq!(
            out.lines().collect::<Vec<_>>(),
            [
                r#"{"input":{"command":"loom guard\nls"}}"#,
                "undefined",
                "undefined",
                "undefined"
            ]
        );
        assert_eq!(
            calls.matches(r#""event":"session_start""#).count(),
            1,
            "{calls}"
        );
        assert_eq!(
            calls.matches(r#""event":"tool_call""#).count(),
            3,
            "{calls}"
        );
        assert!(calls.contains(r#""transcript":"/s.jsonl""#), "{calls}");
        assert!(calls.contains(r#""effort":"high""#), "{calls}");
    }

    #[cfg(unix)]
    #[test]
    fn only_a_finished_main_turn_waits_for_mail() {
        use super::super::pi::tests::{LOG_LINES, run};
        let (out, calls) = run(
            EXTENSION_FILE,
            &format!("{LOG_LINES}answer \"$line\" ',\"text\":\"loom message #1\"'; done"),
            "const sub = { ...ctx, agent: { kind: 'sub' } };
             const said = (text, more) => ({ messages: [{ role: 'assistant', content: [{ type: 'text', text }], ...more }] });
             on.agent_end({ ...said('next'), willContinue: true }, ctx);
             on.agent_end(said('stop', { stopReason: 'aborted' }), ctx);
             on.agent_end(said('sub'), sub);
             on.agent_end({ ...said('done'), willContinue: true, awaitingAsyncWork: true }, ctx);
             await until(() => sent.length, 8000);
             console.log(JSON.stringify(sent));",
        );
        assert!(out.contains("loom message #1"), "{out}");
        assert_eq!(calls.matches(r#""event":"settled""#).count(), 1, "{calls}");
        assert!(calls.contains(r#""reply":"done""#), "{calls}");
    }

    #[cfg(unix)]
    #[test]
    fn an_interrupt_during_a_tool_aborts_the_turn_and_arrives_as_a_message() {
        use super::super::pi::tests::{LOG_LINES, run};
        let (out, calls) = run(
            EXTENSION_FILE,
            &format!(
                "{LOG_LINES}case \"$line\" in *tool_call*) answer \"$line\" ',\"text\":\"loom message #9\"';; *) answer \"$line\";; esac; done"
            ),
            "let aborted = 0;
             const busy = { ...ctx, isIdle: () => aborted > 0, abort: () => aborted++ };
             on.tool_execution_start({}, busy);
             on.tool_execution_start({}, busy);
             await until(() => sent.length, 5000);
             on.tool_execution_end({}, busy);
             console.log(aborted, JSON.stringify(sent.map((m) => m.content)));",
        );
        assert_eq!(out.trim(), r#"1 ["loom message #9"]"#);
        assert_eq!(
            calls.matches(r#""event":"tool_call""#).count(),
            1,
            "{calls}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn tools_that_start_one_after_another_share_one_interrupt_poll() {
        use super::super::pi::tests::{LOG_LINES, run};
        let (_, calls) = run(
            EXTENSION_FILE,
            &format!("{LOG_LINES}answer \"$line\"; done"),
            "const busy = { ...ctx, isIdle: () => false };
             for (let n = 0; n < 6; n++) {
               on.tool_execution_start({}, busy);
               await new Promise((r) => setTimeout(r, 50));
               on.tool_execution_end({}, busy);
             }
             on.tool_execution_start({}, busy);
             await new Promise((r) => setTimeout(r, 3150));
             on.tool_execution_end({}, busy);",
        );
        let polls = calls.matches(r#""event":"tool_call""#).count();
        assert!((2..=3).contains(&polls), "{polls}: {calls}");
    }

    #[test]
    fn every_launch_asks_the_human_through_loom() {
        super::super::assert_every_launch_asks(|launch| {
            let text = super::super::pi::usage(WAITING, launch);
            (text.clone(), text)
        });
    }
}
