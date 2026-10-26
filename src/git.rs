use std::path::Path;

use tokio::process::Command;

use crate::wsl::{self, WslPath};

pub async fn run(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = output(dir, args).await?;
    if !out.status.success() {
        return Err(failure(&out));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub async fn holds(dir: &Path, args: &[&str]) -> Result<bool, String> {
    let out = output(dir, args).await?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) if out.stdout.is_empty() && git_text(&out.stderr).is_empty() => Ok(false),
        _ => Err(failure(&out)),
    }
}

async fn output(dir: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    command(dir, wsl::of(dir).as_ref(), args)
        .output()
        .await
        .map_err(|e| format!("git을 실행하지 못했다: {e}"))
}

fn failure(out: &std::process::Output) -> String {
    [git_text(&out.stderr), git_text(&out.stdout)]
        .into_iter()
        .find(|text| !text.is_empty())
        .unwrap_or_else(|| format!("git이 {}로 끝났다", out.status))
}

fn git_text(text: &[u8]) -> String {
    String::from_utf8_lossy(text)
        .lines()
        .filter(|line| !line.starts_with("wsl: "))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

fn command(dir: &Path, wsl: Option<&WslPath>, args: &[&str]) -> Command {
    let mut cmd = match wsl {
        Some(wsl) => {
            let mut cmd = Command::new("wsl.exe");
            cmd.args(["-d", &wsl.distro, "-e", "git", "-C", &wsl.path]);
            cmd
        }
        None => {
            let mut safe = std::ffi::OsString::from("safe.directory=");
            safe.push(dir);
            let mut cmd = Command::new("git");
            cmd.arg("-c").arg(safe).arg("-C").arg(dir);
            cmd
        }
    };
    cmd.args(args);
    cmd
}

pub fn to_git_path(dir: &Path, path: &str) -> String {
    match (wsl::of(dir), wsl::location(path)) {
        (Some(_), Some(inside)) => inside.path,
        _ => path.to_owned(),
    }
}

pub fn from_git_path(dir: &Path, path: &str) -> String {
    match wsl::of(dir) {
        Some(wsl) if path.starts_with('/') => wsl.unc(path),
        _ => path.to_owned(),
    }
}

pub async fn clone(url: &str, target: &Path) -> Result<(), String> {
    let out = clone_command(url, target)
        .output()
        .await
        .map_err(|e| format!("git을 실행하지 못했다: {e}"))?;
    if !out.status.success() {
        return Err(failure(&out));
    }
    Ok(())
}

fn clone_command(url: &str, target: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(["clone", "--", url])
        .arg(target)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env("GCM_INTERACTIVE", "never")
        .stdin(std::process::Stdio::null());
    crate::timed::own_session(cmd.as_std_mut());
    cmd
}
