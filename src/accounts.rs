use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use portable_pty::CommandBuilder;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::process_tree;
use crate::protocol::{LaunchError, Provider};
use crate::wsl::Inside;

/// 만든 시각(밀리초)의 16진수 16자리. 이름 순서가 만든 순서다.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AccountId(String);

impl AccountId {
    fn checked(&self) -> io::Result<&str> {
        let ok = self.0.len() == 16 && self.0.bytes().all(|b| b.is_ascii_hexdigit());
        ok.then_some(self.0.as_str())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "계정 id 형식이 아니다"))
    }
}

pub struct Accounts {
    root: PathBuf,
    cli_root: Option<String>,
}

impl Accounts {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            cli_root: None,
        }
    }

    pub fn list(&self, provider: Provider) -> io::Result<Vec<AccountId>> {
        let dir = self.root.join(provider_dir(provider));
        let entries = match std::fs::read_dir(&dir) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            entries => entries?,
        };
        let mut ids = Vec::new();
        for entry in entries {
            let id = AccountId(entry?.file_name().to_string_lossy().into_owned());
            if id.checked().is_ok() {
                ids.push(id);
            }
        }
        ids.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(ids)
    }

    pub fn create(&self, provider: Provider) -> io::Result<AccountId> {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_millis();
        let id = AccountId(format!("{millis:016x}"));
        std::fs::create_dir_all(self.root.join(provider_dir(provider)))?;
        std::fs::create_dir(self.dir(provider, &id)?)?;
        Ok(id)
    }

    pub fn remove(&self, provider: Provider, id: &AccountId) -> io::Result<()> {
        std::fs::remove_dir_all(self.dir(provider, id)?)
    }

    pub fn config_dir(&self, provider: Provider, id: Option<&AccountId>) -> Option<PathBuf> {
        id.and_then(|id| self.dir(provider, id).ok())
            .filter(|dir| dir.is_dir())
    }

    fn env(&self, provider: Provider, id: Option<&AccountId>) -> Option<(&'static str, OsString)> {
        let dir = self.config_dir(provider, id)?;
        let value = match (&self.cli_root, id) {
            (Some(root), Some(id)) => format!("{root}/{}/{}", provider_dir(provider), id.0).into(),
            _ => dir.into_os_string(),
        };
        Some((config_env(provider), value))
    }

    fn dir(&self, provider: Provider, id: &AccountId) -> io::Result<PathBuf> {
        Ok(self.root.join(provider_dir(provider)).join(id.checked()?))
    }
}

fn provider_dir(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
    }
}

pub struct Site {
    accounts: Accounts,
    wsl: Option<Inside>,
}

impl Site {
    pub fn pc(root: PathBuf) -> Self {
        Self {
            accounts: Accounts::new(root),
            wsl: None,
        }
    }

    pub fn wsl(inside: Inside) -> Self {
        let cli_root = format!("{}/.loom/accounts", inside.home());
        Self {
            accounts: Accounts {
                root: inside.local(&cli_root),
                cli_root: Some(cli_root),
            },
            wsl: Some(inside),
        }
    }

    pub fn distro(&self) -> Option<&str> {
        self.wsl.as_ref().map(|i| i.distro.as_str())
    }

    pub fn accounts(&self) -> &Accounts {
        &self.accounts
    }

    pub fn inside(&self) -> Option<&Inside> {
        self.wsl.as_ref()
    }

    pub fn into_inside(self) -> Option<Inside> {
        self.wsl
    }

    pub fn local(&self, cli: &str) -> PathBuf {
        match &self.wsl {
            Some(inside) => inside.local(cli),
            None => PathBuf::from(cli),
        }
    }

    pub fn cli_cwd(&self, cwd: &Path) -> String {
        match &self.wsl {
            Some(inside) => inside.cwd().to_owned(),
            None => cwd.to_string_lossy().into_owned(),
        }
    }

    pub fn home_file(&self, rel: &str) -> Option<PathBuf> {
        Some(
            rel.split('/')
                .fold(self.home()?, |path, part| path.join(part)),
        )
    }

    fn home(&self) -> Option<PathBuf> {
        match &self.wsl {
            Some(inside) => Some(inside.local(inside.home())),
            None => std::env::home_dir(),
        }
    }

    pub fn login_dir(&self, provider: Provider, id: Option<&AccountId>) -> Option<PathBuf> {
        self.accounts.config_dir(provider, id).or_else(|| {
            Some(self.home()?.join(match provider {
                Provider::Claude => ".claude",
                Provider::Codex => ".codex",
            }))
        })
    }

    pub fn claude_files(&self, id: Option<&AccountId>) -> Option<(PathBuf, PathBuf)> {
        match self.accounts.config_dir(Provider::Claude, id) {
            Some(dir) => Some((dir.join(".claude.json"), dir.join("settings.json"))),
            None => {
                let home = self.home()?;
                Some((
                    home.join(".claude.json"),
                    home.join(".claude").join("settings.json"),
                ))
            }
        }
    }

    pub fn apply(&self, cmd: &mut CommandBuilder, provider: Provider, id: Option<&AccountId>) {
        if let Some((key, value)) = self.env(provider, id) {
            cmd.env(key, value);
        }
    }

    pub fn env(
        &self,
        provider: Provider,
        id: Option<&AccountId>,
    ) -> Option<(&'static str, OsString)> {
        if provider == Provider::Claude
            && let Some(dir) = self.accounts.config_dir(provider, id)
        {
            let _ = mark_claude_onboarded(&dir);
        }
        self.accounts.env(provider, id)
    }

    pub fn command(
        &self,
        provider: Provider,
        args: &[&str],
        id: Option<&AccountId>,
    ) -> Result<std::process::Command, LaunchError> {
        let program = self.cli(provider)?;
        let env: Vec<_> = self.accounts.env(provider, id).into_iter().collect();
        Ok(match &self.wsl {
            Some(inside) => {
                let argv: Vec<OsString> = std::iter::once(program)
                    .chain(args.iter().map(OsString::from))
                    .collect();
                inside.server(&argv, &env)
            }
            None => {
                let mut cmd = process_tree::command_at(Path::new(&program));
                cmd.args(args);
                cmd.envs(env.iter().map(|(k, v)| (k, v)));
                cmd
            }
        })
    }

    pub fn login(
        &self,
        provider: Provider,
        id: Option<&AccountId>,
    ) -> Result<CommandBuilder, LaunchError> {
        let program = self.cli(provider)?;
        let mut cmd = match &self.wsl {
            Some(_) => CommandBuilder::new(&program),
            None => CommandBuilder::from_argv(process_tree::argv(Path::new(&program))),
        };
        cmd.args(match provider {
            Provider::Claude => &["auth", "login"][..],
            Provider::Codex => &["login"],
        });
        process_tree::color_terminal(&mut cmd);
        self.apply(&mut cmd, provider, id);
        Ok(match &self.wsl {
            Some(inside) => inside.wrap(&cmd),
            None => cmd,
        })
    }

    pub fn logout(&self, provider: Provider) -> Result<std::process::Command, LaunchError> {
        let args: &[&str] = match provider {
            Provider::Claude => &["auth", "logout"],
            Provider::Codex => &["logout"],
        };
        self.command(provider, args, None)
    }

    fn cli(&self, provider: Provider) -> Result<OsString, LaunchError> {
        let (label, command) = match provider {
            Provider::Claude => ("Claude", "claude"),
            Provider::Codex => ("Codex", "codex"),
        };
        self.find(command).ok_or_else(|| LaunchError::NotInstalled {
            agent: label.into(),
            command: command.into(),
            distro: self.distro().map(str::to_owned),
        })
    }

    pub fn installed(&self, command: &str) -> bool {
        self.find(command).is_some()
    }

    fn find(&self, command: &str) -> Option<OsString> {
        match &self.wsl {
            Some(inside) => inside.find(&[command.to_owned()]).map(OsString::from),
            None => crate::search_path::find(command).map(PathBuf::into_os_string),
        }
    }
}

const CLAUDE_ONBOARDED: &str = "hasCompletedOnboarding";

fn mark_claude_onboarded(dir: &Path) -> io::Result<()> {
    update_json(&dir.join(".claude.json"), |config| {
        config.insert(CLAUDE_ONBOARDED.into(), true.into()) != Some(true.into())
    })
}

pub fn carry_claude_consent(from: &(PathBuf, PathBuf), to: &Path, cwd: &str) -> io::Result<()> {
    let (config, settings) = from;
    let trusted = read_json(config)?
        .and_then(|c| c.get("projects")?.get(cwd)?.get(CLAUDE_TRUSTED).cloned())
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
            if project[CLAUDE_TRUSTED] == true {
                return false;
            }
            project[CLAUDE_TRUSTED] = true.into();
            true
        })?;
    }
    let bypass = read_json(settings)?.and_then(|c| c.get(CLAUDE_BYPASS_ACCEPTED).cloned())
        == Some(true.into());
    if bypass {
        update_json(&to.join("settings.json"), |settings| {
            settings.insert(CLAUDE_BYPASS_ACCEPTED.into(), true.into()) != Some(true.into())
        })?;
    }
    Ok(())
}

const CLAUDE_TRUSTED: &str = "hasTrustDialogAccepted";
const CLAUDE_BYPASS_ACCEPTED: &str = "skipDangerousModePermissionPrompt";

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

fn config_env(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "CLAUDE_CONFIG_DIR",
        Provider::Codex => "CODEX_HOME",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_list_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let accounts = Accounts::new(tmp.path().to_path_buf());
        assert!(accounts.list(Provider::Claude).unwrap().is_empty());
        let id = accounts.create(Provider::Claude).unwrap();
        assert_eq!(accounts.list(Provider::Claude).unwrap(), vec![id.clone()]);
        assert!(accounts.list(Provider::Codex).unwrap().is_empty());
        assert!(accounts.config_dir(Provider::Claude, Some(&id)).is_some());
        accounts.remove(Provider::Claude, &id).unwrap();
        assert!(accounts.config_dir(Provider::Claude, Some(&id)).is_none());
    }

    #[test]
    fn claude_account_skips_first_run_guide() {
        let tmp = tempfile::tempdir().unwrap();
        let site = Site::pc(tmp.path().to_path_buf());
        let accounts = site.accounts();
        let id = accounts.create(Provider::Claude).unwrap();
        let file = accounts
            .config_dir(Provider::Claude, Some(&id))
            .unwrap()
            .join(".claude.json");
        std::fs::write(&file, r#"{"oauthAccount":{"emailAddress":"a@b.c"}}"#).unwrap();
        let mut cmd = CommandBuilder::new("claude");
        site.apply(&mut cmd, Provider::Claude, Some(&id));
        assert_eq!(
            cmd.get_env("CLAUDE_CONFIG_DIR"),
            Some(file.parent().unwrap().as_os_str())
        );
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(config["hasCompletedOnboarding"], true);
        assert_eq!(config["oauthAccount"]["emailAddress"], "a@b.c");

        let codex = accounts.create(Provider::Codex).unwrap();
        site.apply(
            &mut CommandBuilder::new("codex"),
            Provider::Codex,
            Some(&codex),
        );
        let dir = accounts.config_dir(Provider::Codex, Some(&codex)).unwrap();
        assert!(!dir.join(".claude.json").exists());
    }

    #[test]
    fn wsl_account_env_is_the_linux_path() {
        let tmp = tempfile::tempdir().unwrap();
        let accounts = Accounts {
            root: tmp.path().to_path_buf(),
            cli_root: Some("/home/me/.loom/accounts".into()),
        };
        let id = accounts.create(Provider::Codex).unwrap();
        assert_eq!(
            accounts.env(Provider::Codex, Some(&id)),
            Some((
                "CODEX_HOME",
                format!("/home/me/.loom/accounts/codex/{}", id.0).into()
            ))
        );
        assert_eq!(accounts.env(Provider::Codex, None), None);
    }

    #[test]
    fn consent_moves_only_what_the_old_folder_accepted() {
        let tmp = tempfile::tempdir().unwrap();
        let (old, new) = (tmp.path().join("old"), tmp.path().join("new"));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(
            old.join(".claude.json"),
            r#"{"projects":{"/w":{"hasTrustDialogAccepted":true},"/x":{"hasTrustDialogAccepted":false}}}"#,
        )
        .unwrap();
        std::fs::write(new.join(".claude.json"), r#"{"oauthAccount":{"a":1}}"#).unwrap();
        let from = (old.join(".claude.json"), old.join("settings.json"));
        carry_claude_consent(&from, &new, "/w").unwrap();
        carry_claude_consent(&from, &new, "/x").unwrap();
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(new.join(".claude.json")).unwrap()).unwrap();
        assert_eq!(config["projects"]["/w"]["hasTrustDialogAccepted"], true);
        assert!(config["projects"].get("/x").is_none());
        assert_eq!(config["oauthAccount"]["a"], 1);
        assert!(!new.join("settings.json").exists());
        std::fs::write(
            old.join("settings.json"),
            r#"{"skipDangerousModePermissionPrompt":true}"#,
        )
        .unwrap();
        carry_claude_consent(&from, &new, "/w").unwrap();
        let settings: serde_json::Value =
            serde_json::from_slice(&std::fs::read(new.join("settings.json")).unwrap()).unwrap();
        assert_eq!(settings["skipDangerousModePermissionPrompt"], true);
    }

    #[test]
    fn rejects_ids_that_leave_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        let accounts = Accounts::new(tmp.path().join("accounts"));
        let escape = AccountId("../../etc".into());
        assert!(accounts.remove(Provider::Claude, &escape).is_err());
        assert!(
            accounts
                .config_dir(Provider::Claude, Some(&escape))
                .is_none()
        );
    }
}
