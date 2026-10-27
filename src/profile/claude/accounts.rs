use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::accounts::{AccountId, Cli, Site};
use crate::protocol::Provider;

pub static CLI: Cli = Cli {
    provider: Provider::Claude,
    label: "Claude",
    command: "claude",
    folder: "claude",
    home: ".claude",
    env: "CLAUDE_CONFIG_DIR",
    login: &["auth", "login"],
    logout: &["auth", "logout"],
    ready,
};

fn ready(dir: &Path, site: &Site) {
    let _ = mark_onboarded(dir);
    if let Some(home_md) = site.home_file(HOME_CLAUDE_MD) {
        link_claude_md(&home_md, dir);
    }
}

const ONBOARDED: &str = "hasCompletedOnboarding";

fn mark_onboarded(dir: &Path) -> io::Result<()> {
    update_json(&dir.join(".claude.json"), |config| {
        config.insert(ONBOARDED.into(), true.into()) != Some(true.into())
    })
}

const HOME_CLAUDE_MD: &str = ".claude/CLAUDE.md";
const CLAUDE_MD_IMPORT: &str = "@~/.claude/CLAUDE.md\n";

fn link_claude_md(home_md: &Path, dir: &Path) {
    if !home_md.is_file() {
        return;
    }
    let file = dir.join("CLAUDE.md");
    let created = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&file)
        .and_then(|mut f| f.write_all(CLAUDE_MD_IMPORT.as_bytes()));
    match created {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            if std::fs::read_to_string(&file).ok().as_deref() != Some(CLAUDE_MD_IMPORT) {
                eprintln!(
                    "loom: {}이 이미 있어 ~/.claude/CLAUDE.md를 잇지 않는다",
                    file.display()
                );
            }
        }
        Err(e) => eprintln!("loom: {}을 쓰지 못했다: {e}", file.display()),
    }
}

pub fn consent_files(site: &Site, id: Option<&AccountId>) -> Option<(PathBuf, PathBuf)> {
    match site.accounts().config_dir(&CLI, id) {
        Some(dir) => Some((dir.join(".claude.json"), dir.join("settings.json"))),
        None => Some((
            site.home_file(".claude.json")?,
            site.home_file(".claude/settings.json")?,
        )),
    }
}

pub fn carry_consent(from: &(PathBuf, PathBuf), to: &Path, cwd: &str) -> io::Result<()> {
    let (config, settings) = from;
    let trusted = read_json(config)?
        .and_then(|c| c.get("projects")?.get(cwd)?.get(TRUSTED).cloned())
        == Some(true.into());
    if trusted {
        update_json(&to.join(".claude.json"), |config| {
            let projects = config
                .entry("projects")
                .or_insert_with(|| serde_json::json!({}));
            let Some(project) = projects
                .as_object_mut()
                .map(|p| p.entry(cwd).or_insert_with(|| serde_json::json!({})))
            else {
                return false;
            };
            if project[TRUSTED] == true {
                return false;
            }
            project[TRUSTED] = true.into();
            true
        })?;
    }
    let bypass =
        read_json(settings)?.and_then(|c| c.get(BYPASS_ACCEPTED).cloned()) == Some(true.into());
    if bypass {
        update_json(&to.join("settings.json"), |settings| {
            settings.insert(BYPASS_ACCEPTED.into(), true.into()) != Some(true.into())
        })?;
    }
    Ok(())
}

const TRUSTED: &str = "hasTrustDialogAccepted";
const BYPASS_ACCEPTED: &str = "skipDangerousModePermissionPrompt";

fn read_json(path: &Path) -> io::Result<Option<serde_json::Map<String, serde_json::Value>>> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(io::Error::other),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn update_json(
    path: &Path,
    change: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>) -> bool,
) -> io::Result<()> {
    let mut config = read_json(path)?.unwrap_or_default();
    if change(&mut config) {
        std::fs::write(path, serde_json::to_vec_pretty(&config)?)?;
    }
    Ok(())
}
