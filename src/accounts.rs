//! 계정: 위치(이 PC, WSL 배포판)와 공급자마다 loom이 만든 로그인 폴더(`~/.loom/accounts/<공급자>/<id>`).
//! 「시스템 기본」은 그 위치의 `~/.claude`·`~/.codex`다. loom은 사람이 [로그인]·[로그아웃]을 누를 때만 CLI로 그 폴더를 바꾼다.
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
use crate::wsl::Inside;

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
    /// 데몬이 읽고 쓰는 경로. WSL이면 UNC 경로다.
    root: PathBuf,
    /// CLI가 보는 경로. WSL이면 배포판 안의 Linux 경로, 이 PC면 없다(`root`와 같다).
    cli_root: Option<String>,
}

impl Accounts {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            cli_root: None,
        }
    }

    /// 만든 순서로 돌려준다.
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
        // 이미 있으면 실패한다. 같은 밀리초에 두 번 누른 경우다.
        std::fs::create_dir(self.dir(provider, &id)?)?;
        Ok(id)
    }

    pub fn remove(&self, provider: Provider, id: &AccountId) -> io::Result<()> {
        std::fs::remove_dir_all(self.dir(provider, id)?)
    }

    /// 데몬이 읽을 로그인 폴더. `None`이거나 폴더가 사라졌으면 시스템 기본이다.
    pub fn config_dir(&self, provider: Provider, id: Option<&AccountId>) -> Option<PathBuf> {
        id.and_then(|id| self.dir(provider, id).ok())
            .filter(|dir| dir.is_dir())
    }

    /// 로그인 폴더를 CLI에 알리는 환경 변수. 시스템 기본이면 없다.
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

/// 계정과 CLI가 있는 위치: 이 PC 또는 WSL 배포판 하나. 위치는 저장소 경로가 정한다(`wsl::of`).
/// 위치마다 「시스템 기본」과 loom 계정이 따로 있다. 근거: docs/design.md 「계정과 사용량」, 목업 docs/mockups/wsl-place.html ③.
pub struct Site {
    accounts: Accounts,
    /// WSL이면 그 배포판에서 읽은 값. CLI를 그 배포판에서 띄운다.
    wsl: Option<Inside>,
}

impl Site {
    /// 이 PC. loom 계정은 `root`(데몬 데이터 폴더의 `accounts`)에 있다.
    pub fn pc(root: PathBuf) -> Self {
        Self {
            accounts: Accounts::new(root),
            wsl: None,
        }
    }

    /// WSL 배포판. loom 계정은 그 배포판의 `~/.loom/accounts`에 있다. 그 배포판의 Linux loom이 만든 계정도 그대로 보인다.
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

    /// WSL이면 배포판 이름, 이 PC면 없다.
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

    /// CLI가 본 경로(예: 세션 기록 파일)를 데몬이 읽는 경로로 바꾼다.
    pub fn local(&self, cli: &str) -> PathBuf {
        match &self.wsl {
            Some(inside) => inside.local(cli),
            None => PathBuf::from(cli),
        }
    }

    /// 노드가 CLI에 넘기는 작업 폴더 글자. Claude가 폴더 신뢰를 이 글자로 적는다.
    pub fn cli_cwd(&self, cwd: &Path) -> String {
        match &self.wsl {
            Some(inside) => inside.cwd().to_owned(),
            None => cwd.to_string_lossy().into_owned(),
        }
    }

    /// 이 위치의 홈. 시스템 기본 로그인 폴더가 그 아래에 있다.
    fn home(&self) -> Option<PathBuf> {
        match &self.wsl {
            Some(inside) => Some(inside.local(inside.home())),
            None => std::env::home_dir(),
        }
    }

    /// 로그인 폴더(데몬이 읽는 경로): loom 계정이면 그 폴더, 시스템 기본이면 CLI가 환경 변수 없이 쓰는 폴더다.
    pub fn login_dir(&self, provider: Provider, id: Option<&AccountId>) -> Option<PathBuf> {
        self.accounts.config_dir(provider, id).or_else(|| {
            Some(self.home()?.join(match provider {
                Provider::Claude => ".claude",
                Provider::Codex => ".codex",
            }))
        })
    }

    /// Claude가 사람의 확인을 적는 두 파일: `.claude.json`(폴더 신뢰)과 `settings.json`(모든 권한 허용 경고).
    /// 시스템 기본은 `~/.claude.json`과 `~/.claude/settings.json`이다.
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

    /// 로그인 폴더를 CLI에 알리는 환경 변수를 `cmd`에 넣는다. 시스템 기본이면 넣지 않는다.
    /// Claude 폴더에는 첫 실행 안내를 마쳤다고 적는다(`mark_claude_onboarded`).
    pub fn apply(&self, cmd: &mut CommandBuilder, provider: Provider, id: Option<&AccountId>) {
        if let Some((key, value)) = self.env(provider, id) {
            cmd.env(key, value);
        }
    }

    /// `apply`와 같다. 노드 서버처럼 `CommandBuilder`가 아닌 명령에 넣을 값이다.
    pub fn env(
        &self,
        provider: Provider,
        id: Option<&AccountId>,
    ) -> Option<(&'static str, OsString)> {
        if provider == Provider::Claude
            && let Some(dir) = self.accounts.config_dir(provider, id)
        {
            // 적지 못하면 Claude가 첫 실행 안내를 보인다. 노드는 그대로 띄운다.
            let _ = mark_claude_onboarded(&dir);
        }
        self.accounts.env(provider, id)
    }

    /// 이 위치에서 CLI를 띄우는 명령. 로그인 폴더 변수(`login`)도 넣는다. CLI가 없으면 노드 만들기와 같은 오류다.
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
                let mut argv = process_tree::argv(Path::new(&program)).into_iter();
                let mut cmd = std::process::Command::new(argv.next().expect("argv는 비지 않는다"));
                cmd.args(argv).args(args);
                cmd.envs(env.iter().map(|(k, v)| (k, v)));
                cmd
            }
        })
    }

    /// CLI 로그인 창 명령. `id`가 없으면 시스템 기본 폴더에 로그인한다.
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
        profile::color_terminal(&mut cmd);
        self.apply(&mut cmd, provider, id);
        Ok(match &self.wsl {
            Some(inside) => inside.wrap(&cmd),
            None => cmd,
        })
    }

    /// 시스템 기본 폴더의 로그인을 지우는 명령. loom이 만든 계정은 폴더를 지운다(`Accounts::remove`).
    pub fn logout(&self, provider: Provider) -> Result<std::process::Command, LaunchError> {
        let args: &[&str] = match provider {
            Provider::Claude => &["auth", "logout"],
            Provider::Codex => &["logout"],
        };
        self.command(provider, args, None)
    }

    /// 노드를 띄울 때처럼 CLI를 찾는다: 이 PC는 데몬 `PATH`, WSL은 그 배포판의 `PATH`.
    /// Windows npm 배치 파일(`codex.cmd`)은 `command`·`login`이 그 프로그램을 바로 띄운다(`process_tree::argv`).
    fn cli(&self, provider: Provider) -> Result<OsString, LaunchError> {
        let (label, command) = match provider {
            Provider::Claude => ("Claude", "claude"),
            Provider::Codex => ("Codex", "codex"),
        };
        let found = match &self.wsl {
            Some(inside) => inside.find(&[command.to_owned()]).map(OsString::from),
            None => which::which(command).ok().map(PathBuf::into_os_string),
        };
        found.ok_or_else(|| LaunchError::NotInstalled {
            agent: label.into(),
            command: command.into(),
            distro: self.distro().map(str::to_owned),
        })
    }
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
/// `from`은 옛 폴더의 두 파일이다(`Site::claude_files`). `cwd`는 CLI가 본 작업 폴더 글자다. 시스템 기본에는 쓰지 않는다.
/// 옛 폴더에 없는 값은 옮기지 않는다. 새 폴더에 이미 있으면 쓰지 않는다.
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

    /// WSL 계정 폴더는 데몬이 UNC로 읽고, CLI에는 배포판 안의 Linux 경로로 알린다.
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
        // 옛 폴더가 경고를 받아들이지 않았으면 옮기지 않는다.
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
