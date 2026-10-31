use std::path::Path;
use std::time::Duration;

use tokio::process::Command;

use crate::distro::{self, Ran};
use crate::place::{self, Files, Machine};

const REMOTE_CWD: &str = "/";
const GIT_WAIT: Duration = Duration::from_secs(5 * 60);
const CLONE_WAIT: Duration = Duration::from_secs(60 * 60);

pub async fn run(files: Files<'_>, dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = output(files, dir, args).await?;
    succeeded(&out)?;
    Ok(out.stdout)
}

pub async fn holds(files: Files<'_>, dir: &Path, args: &[&str]) -> Result<bool, String> {
    let out = output(files, dir, args).await?;
    match out.code {
        Some(0) => Ok(true),
        Some(1) if out.stdout.is_empty() && git_text(&out.stderr).is_empty() => Ok(false),
        _ => Err(failure(&out)),
    }
}

async fn output(files: Files<'_>, dir: &Path, args: &[&str]) -> Result<Ran, String> {
    let mut cmd = match place::of(dir) {
        Machine::Wsl(wsl) => {
            let mut cmd = Command::new("wsl.exe");
            cmd.args(["-d", &wsl.distro, "-e", "git", "-C", &wsl.path]);
            cmd
        }
        Machine::Pc => {
            let mut safe = std::ffi::OsString::from("safe.directory=");
            safe.push(dir);
            let mut cmd = Command::new("git");
            cmd.arg("-c").arg(safe).arg("-C").arg(dir);
            cmd
        }
        Machine::Ssh(at) => {
            let argv = ["git", "-C", &at.path]
                .into_iter()
                .chain(args.iter().copied());
            return files.run(&at.host, remote(argv, &[]), GIT_WAIT).await;
        }
    };
    cmd.args(args);
    ran(cmd, GIT_WAIT).await
}

pub fn remote<'a>(
    argv: impl IntoIterator<Item = &'a str>,
    env: &[(&str, &str)],
) -> distro::Command {
    distro::Command {
        argv: argv.into_iter().map(str::to_owned).collect(),
        cwd: REMOTE_CWD.to_owned(),
        env: env
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
    }
}

async fn ran(cmd: Command, limit: Duration) -> Result<Ran, String> {
    let out = crate::timed::ended_within(cmd, limit)
        .await
        .map_err(|e| format!("git: {e}"))?;
    Ok(Ran {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

fn failure(out: &Ran) -> String {
    [git_text(&out.stderr), git_text(&out.stdout)]
        .into_iter()
        .find(|text| !text.is_empty())
        .unwrap_or_else(|| match out.code {
            Some(code) => format!("git이 코드 {code}로 끝났다"),
            None => "git이 신호로 끝났다".to_owned(),
        })
}

fn git_text(text: &str) -> String {
    text.lines()
        .filter(|line| !line.starts_with("wsl: "))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

pub fn to_git_path(dir: &Path, path: &str) -> String {
    match place::of(dir) {
        Machine::Pc => path.to_owned(),
        Machine::Wsl(_) | Machine::Ssh(_) => place::inner(path),
    }
}

pub fn from_git_path(dir: &Path, path: &str) -> String {
    place::of(dir).local(path)
}

const NO_PROMPT: [(&str, &str); 4] = [
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_ASKPASS", ""),
    ("SSH_ASKPASS_REQUIRE", "never"),
    ("GCM_INTERACTIVE", "never"),
];

pub async fn clone(files: Files<'_>, url: &str, target: &Path) -> Result<(), String> {
    let mut cmd = match place::of(target) {
        Machine::Wsl(at) => {
            let mut cmd = Command::new("wsl.exe");
            cmd.args(["-d", &at.distro, "-e", "git", "clone", "--", url, &at.path])
                .env("WSLENV", NO_PROMPT.map(|(name, _)| name).join(":"));
            cmd
        }
        Machine::Pc => {
            let mut cmd = Command::new("git");
            cmd.args(["clone", "--", url]).arg(target);
            cmd
        }
        Machine::Ssh(at) => {
            let argv = ["git", "clone", "--", url, &at.path];
            let out = files
                .run(&at.host, remote(argv, &NO_PROMPT), CLONE_WAIT)
                .await?;
            return succeeded(&out);
        }
    };
    cmd.envs(NO_PROMPT)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY");
    succeeded(&ran(cmd, CLONE_WAIT).await?)
}

fn succeeded(out: &Ran) -> Result<(), String> {
    match out.code {
        Some(0) => Ok(()),
        _ => Err(failure(out)),
    }
}
