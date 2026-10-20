//! 계정: 공급자마다 loom이 만든 로그인 폴더(`~/.loom/accounts/<공급자>/<id>`).
//! 「시스템 기본」은 사용자의 `~/.claude`·`~/.codex`다. loom은 그 폴더를 고치지 않는다.
//! 근거: docs/design.md 「계정과 사용량」.

use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use portable_pty::CommandBuilder;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::profile::{self, Provider};

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
    pub fn apply(&self, cmd: &mut CommandBuilder, provider: Provider, id: Option<&AccountId>) {
        if let Some(dir) = self.config_dir(provider, id) {
            cmd.env(config_env(provider), dir);
        }
    }

    /// 그 계정 폴더로 CLI 로그인을 띄우는 명령.
    pub fn login(&self, provider: Provider, id: &AccountId) -> io::Result<CommandBuilder> {
        let mut cmd = match provider {
            Provider::Claude => {
                let mut cmd = CommandBuilder::new("claude");
                cmd.args(["auth", "login"]);
                cmd
            }
            Provider::Codex => {
                let mut cmd = CommandBuilder::new("codex");
                cmd.arg("login");
                cmd
            }
        };
        cmd.env(config_env(provider), self.dir(provider, id)?);
        profile::color_terminal(&mut cmd);
        Ok(cmd)
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
