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
