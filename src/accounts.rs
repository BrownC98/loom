//! 계정: 공급자마다 loom이 만든 로그인 폴더(`~/.loom/accounts/<공급자>/<id>`).
//! 「시스템 기본」은 사용자의 `~/.claude`·`~/.codex`다. loom은 사람이 [로그인]·[로그아웃]을 누를 때만 CLI로 그 폴더를 바꾼다.
//! 근거: docs/design.md 「계정과 사용량」.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use portable_pty::CommandBuilder;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::process_tree;
use crate::profile::{self, LaunchError, Provider};

/// 만든 시각(밀리초)의 16진수 16자리. 이름 순서가 만든 순서다.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AccountId(String);

impl AccountId {
    /// 경로 조각으로 써도 안전한 id만 받는다.
    fn checked(&self) -> io::Result<&str> {
        let ok = self.0.len() == 16 && self.0.bytes().all(|b| b.is_ascii_hexdigit());
        ok.then_some(self.0.as_str())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "계정 id 형식이 아니다"))
    }
}

pub struct Accounts {
    root: PathBuf,
}

impl Accounts {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// 만든 순서로 돌려준다.
    pub fn list(&self, provider: Provider) -> io::Result<Vec<AccountId>> {
        let dir = self.provider_dir(provider);
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
        std::fs::create_dir_all(self.provider_dir(provider))?;
        // 이미 있으면 실패한다. 같은 밀리초에 두 번 누른 경우다.
        std::fs::create_dir(self.dir(provider, &id)?)?;
        Ok(id)
    }

    pub fn remove(&self, provider: Provider, id: &AccountId) -> io::Result<()> {
        std::fs::remove_dir_all(self.dir(provider, id)?)
    }

    /// CLI가 읽을 로그인 폴더. `None`이거나 폴더가 사라졌으면 시스템 기본이다.
    pub fn config_dir(&self, provider: Provider, id: Option<&AccountId>) -> Option<PathBuf> {
        id.and_then(|id| self.dir(provider, id).ok())
            .filter(|dir| dir.is_dir())
    }

    /// 로그인 폴더를 CLI에 알리는 환경 변수를 `cmd`에 넣는다. 시스템 기본이면 넣지 않는다.
    /// Claude 폴더에는 첫 실행 안내를 마쳤다고 적는다(`mark_claude_onboarded`).
    pub fn apply(&self, cmd: &mut CommandBuilder, provider: Provider, id: Option<&AccountId>) {
        if let Some(dir) = self.config_dir(provider, id) {
            if provider == Provider::Claude {
                // 적지 못하면 Claude가 첫 실행 안내를 보인다. 노드는 그대로 띄운다.
                let _ = mark_claude_onboarded(&dir);
            }
            cmd.env(config_env(provider), dir);
        }
    }

    fn provider_dir(&self, provider: Provider) -> PathBuf {
        self.root.join(match provider {
            Provider::Claude => "claude",
            Provider::Codex => "codex",
        })
    }

    fn dir(&self, provider: Provider, id: &AccountId) -> io::Result<PathBuf> {
        Ok(self.provider_dir(provider).join(id.checked()?))
    }
}

/// CLI 로그인을 띄우는 명령. 시스템 기본 폴더에 로그인한다. loom 계정은 `Accounts::apply`로 폴더를 넣는다.
pub fn login(provider: Provider) -> Result<CommandBuilder, LaunchError> {
    let mut cmd = CommandBuilder::from_argv(cli(provider)?);
    cmd.args(match provider {
        Provider::Claude => &["auth", "login"][..],
        Provider::Codex => &["login"],
    });
    profile::color_terminal(&mut cmd);
    Ok(cmd)
}

/// 시스템 기본 폴더의 로그인을 지우는 명령. loom이 만든 계정은 폴더를 지운다(`Accounts::remove`).
pub fn logout(provider: Provider) -> Result<std::process::Command, LaunchError> {
    let mut argv = cli(provider)?.into_iter();
    let mut cmd = std::process::Command::new(argv.next().expect("argv는 비지 않는다"));
    cmd.args(argv);
    cmd.args(match provider {
        Provider::Claude => &["auth", "logout"][..],
        Provider::Codex => &["logout"],
    });
    Ok(cmd)
}

/// 노드를 띄울 때처럼 `PATH`에서 CLI를 찾는다. Windows npm 배치 파일(`codex.cmd`)은 그 프로그램을 바로 띄운다.
fn cli(provider: Provider) -> Result<Vec<OsString>, LaunchError> {
    let (label, command) = match provider {
        Provider::Claude => ("Claude", "claude"),
        Provider::Codex => ("Codex", "codex"),
    };
    let path = which::which(command).map_err(|_| LaunchError::NotInstalled {
        agent: label.into(),
        command: command.into(),
    })?;
    Ok(process_tree::argv(&path))
}

/// Claude 설정(`.claude.json`)에서 첫 실행 안내를 마쳤다는 키.
const CLAUDE_ONBOARDED: &str = "hasCompletedOnboarding";

/// loom 계정 폴더의 `.claude.json`에 `hasCompletedOnboarding: true`를 넣는다.
/// Claude 2.1.228은 이 값이 없으면 로그인한 폴더에서도 첫 실행에 로그인 방법을 다시 묻는다. 2.1.305는 묻지 않는다(2026-10-21 실측).
/// 값이 이미 있으면 쓰지 않는다. 그 폴더로 도는 노드가 쓰는 파일을 덮지 않기 위해서다.
fn mark_claude_onboarded(dir: &Path) -> io::Result<()> {
    update_json(&dir.join(".claude.json"), |config| {
        config.insert(CLAUDE_ONBOARDED.into(), true.into()) != Some(true.into())
    })
}

/// 옛 Claude 로그인 폴더에서 사람이 이미 받아들인 확인을 loom 계정 폴더 `to`로 옮긴다: `cwd` 폴더 신뢰와 모든 권한 허용 경고.
/// 계정 전환으로 다시 띄운 노드가 사람 없이 그 확인 창에서 멈추지 않게 한다(2.1.306 실측: 새 계정 폴더는 두 창을 다시 보인다).
/// `from`이 `None`이면 시스템 기본(`~/.claude.json`, `~/.claude/settings.json`)에서 읽는다. 시스템 기본에는 쓰지 않는다.
/// 옛 폴더에 없는 값은 옮기지 않는다. 새 폴더에 이미 있으면 쓰지 않는다.
pub fn carry_claude_consent(from: Option<&Path>, to: &Path, cwd: &Path) -> io::Result<()> {
    let home = std::env::home_dir().unwrap_or_default();
    let (config, settings) = match from {
        Some(dir) => (dir.join(".claude.json"), dir.join("settings.json")),
        None => (
            home.join(".claude.json"),
            home.join(".claude/settings.json"),
        ),
    };
    let cwd = cwd.to_string_lossy();
    let trusted = read_json(&config)?.and_then(|c| {
        c.get("projects")?
            .get(cwd.as_ref())?
            .get(CLAUDE_TRUSTED)
            .cloned()
    }) == Some(true.into());
    if trusted {
        update_json(&to.join(".claude.json"), |config| {
            let projects = config
                .entry("projects")
                .or_insert_with(|| serde_json::json!({}));
            let Some(project) = projects.as_object_mut().map(|p| {
                p.entry(cwd.as_ref())
                    .or_insert_with(|| serde_json::json!({}))
            }) else {
                return false;
            };
            if project[CLAUDE_TRUSTED] == true {
                return false;
            }
            project[CLAUDE_TRUSTED] = true.into();
            true
        })?;
    }
    let bypass = read_json(&settings)?.and_then(|c| c.get(CLAUDE_BYPASS_ACCEPTED).cloned())
        == Some(true.into());
    if bypass {
        update_json(&to.join("settings.json"), |settings| {
            settings.insert(CLAUDE_BYPASS_ACCEPTED.into(), true.into()) != Some(true.into())
        })?;
    }
    Ok(())
}

/// `.claude.json`의 폴더별 신뢰 키와 `settings.json`의 모든 권한 허용 경고를 받아들였다는 키(2.1.306).
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

/// JSON 객체 파일을 읽어 `change`로 고친다. `change`가 거짓이면 쓰지 않는다. 파일이 없으면 빈 객체에서 시작한다.
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

/// CLI가 로그인 폴더를 고르는 환경 변수.
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
        let accounts = Accounts::new(tmp.path().to_path_buf());
        let id = accounts.create(Provider::Claude).unwrap();
        let file = accounts
            .config_dir(Provider::Claude, Some(&id))
            .unwrap()
            .join(".claude.json");
        std::fs::write(&file, r#"{"oauthAccount":{"emailAddress":"a@b.c"}}"#).unwrap();
        accounts.apply(
            &mut CommandBuilder::new("claude"),
            Provider::Claude,
            Some(&id),
        );
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(config["hasCompletedOnboarding"], true);
        assert_eq!(config["oauthAccount"]["emailAddress"], "a@b.c");

        let codex = accounts.create(Provider::Codex).unwrap();
        accounts.apply(
            &mut CommandBuilder::new("codex"),
            Provider::Codex,
            Some(&codex),
        );
        let dir = accounts.config_dir(Provider::Codex, Some(&codex)).unwrap();
        assert!(!dir.join(".claude.json").exists());
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
        carry_claude_consent(Some(&old), &new, Path::new("/w")).unwrap();
        carry_claude_consent(Some(&old), &new, Path::new("/x")).unwrap();
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(new.join(".claude.json")).unwrap()).unwrap();
        assert_eq!(config["projects"]["/w"]["hasTrustDialogAccepted"], true);
        assert!(config["projects"].get("/x").is_none());
        assert_eq!(config["oauthAccount"]["a"], 1);
        // 옛 폴더가 경고를 받아들이지 않았으면 옮기지 않는다.
        assert!(!new.join("settings.json").exists());
        std::fs::write(
            old.join("settings.json"),
            r#"{"skipDangerousModePermissionPrompt":true}"#,
        )
        .unwrap();
        carry_claude_consent(Some(&old), &new, Path::new("/w")).unwrap();
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
