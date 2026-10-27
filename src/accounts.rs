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

pub struct Cli {
    pub provider: Provider,
    pub label: &'static str,
    pub command: &'static str,
    pub folder: &'static str,
    pub home: &'static str,
    pub env: &'static str,
    pub login: &'static [&'static str],
    pub logout: &'static [&'static str],
    pub ready: fn(&Path, &Site),
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

    pub fn list(&self, cli: &Cli) -> io::Result<Vec<AccountId>> {
        let dir = self.root.join(cli.folder);
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

    pub fn create(&self, cli: &Cli) -> io::Result<AccountId> {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_millis();
        let id = AccountId(format!("{millis:016x}"));
        std::fs::create_dir_all(self.root.join(cli.folder))?;
        std::fs::create_dir(self.dir(cli, &id)?)?;
        Ok(id)
    }

    pub fn remove(&self, cli: &Cli, id: &AccountId) -> io::Result<()> {
        std::fs::remove_dir_all(self.dir(cli, id)?)
    }

    pub fn config_dir(&self, cli: &Cli, id: Option<&AccountId>) -> Option<PathBuf> {
        id.and_then(|id| self.dir(cli, id).ok())
            .filter(|dir| dir.is_dir())
    }

    fn env(&self, cli: &Cli, id: Option<&AccountId>) -> Option<(&'static str, OsString)> {
        let dir = self.config_dir(cli, id)?;
        let value = match (&self.cli_root, id) {
            (Some(root), Some(id)) => format!("{root}/{}/{}", cli.folder, id.0).into(),
            _ => dir.into_os_string(),
        };
        Some((cli.env, value))
    }

    fn dir(&self, cli: &Cli, id: &AccountId) -> io::Result<PathBuf> {
        Ok(self.root.join(cli.folder).join(id.checked()?))
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
        self.wsl.as_ref().map(|i| i.distro.name())
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

    pub fn login_dir(&self, cli: &Cli, id: Option<&AccountId>) -> Option<PathBuf> {
        self.accounts
            .config_dir(cli, id)
            .or_else(|| Some(self.home()?.join(cli.home)))
    }

    pub fn apply(&self, cmd: &mut CommandBuilder, cli: &Cli, id: Option<&AccountId>) {
        if let Some((key, value)) = self.env(cli, id) {
            cmd.env(key, value);
        }
    }

    pub fn env(&self, cli: &Cli, id: Option<&AccountId>) -> Option<(&'static str, OsString)> {
        if let Some(dir) = self.accounts.config_dir(cli, id) {
            (cli.ready)(&dir, self);
        }
        self.accounts.env(cli, id)
    }

    pub fn command(
        &self,
        cli: &Cli,
        args: &[&str],
        id: Option<&AccountId>,
    ) -> Result<std::process::Command, LaunchError> {
        let program = self.program(cli)?;
        let env: Vec<_> = self.accounts.env(cli, id).into_iter().collect();
        Ok(self.run(program, args, &env))
    }

    pub fn installed_command(&self, command: &str, args: &[&str]) -> Option<std::process::Command> {
        Some(self.run(self.find(command)?, args, &[]))
    }

    fn run(
        &self,
        program: OsString,
        args: &[&str],
        env: &[(&'static str, OsString)],
    ) -> std::process::Command {
        match &self.wsl {
            Some(inside) => {
                let argv: Vec<OsString> = std::iter::once(program)
                    .chain(args.iter().map(OsString::from))
                    .collect();
                inside.wsl_command(&argv, env)
            }
            None => {
                let mut cmd = process_tree::command_at(Path::new(&program));
                cmd.args(args);
                cmd.envs(env.iter().map(|(k, v)| (k, v)));
                cmd
            }
        }
    }

    pub fn login(&self, cli: &Cli, id: Option<&AccountId>) -> Result<CommandBuilder, LaunchError> {
        let program = self.program(cli)?;
        let mut cmd = match &self.wsl {
            Some(_) => CommandBuilder::new(&program),
            None => CommandBuilder::from_argv(process_tree::argv(Path::new(&program))),
        };
        cmd.args(cli.login);
        process_tree::color_terminal(&mut cmd);
        self.apply(&mut cmd, cli, id);
        Ok(match &self.wsl {
            Some(inside) => inside.wrap(&cmd),
            None => cmd,
        })
    }

    pub fn logout(&self, cli: &Cli) -> Result<std::process::Command, LaunchError> {
        self.command(cli, cli.logout, None)
    }

    fn program(&self, cli: &Cli) -> Result<OsString, LaunchError> {
        self.find(cli.command)
            .ok_or_else(|| LaunchError::NotInstalled {
                agent: cli.label.into(),
                command: cli.command.into(),
                distro: self.distro().map(str::to_owned),
            })
    }

    fn find(&self, command: &str) -> Option<OsString> {
        match &self.wsl {
            Some(inside) => inside.find(&[command.to_owned()]).map(OsString::from),
            None => crate::search_path::find(command).map(PathBuf::into_os_string),
        }
    }
}
