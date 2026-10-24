use std::path::Path;

use tokio::process::Command;

use crate::process_tree;
use crate::wsl::{self, WslPath};

#[cfg(test)]
tokio::task_local! {
    static CALLS: std::cell::Cell<usize>;
}

#[cfg(test)]
pub async fn calls_during<T>(work: impl Future<Output = T>) -> (usize, T) {
    CALLS
        .scope(std::cell::Cell::new(0), async {
            let value = work.await;
            (CALLS.with(|n| n.get()), value)
        })
        .await
}

pub async fn run(dir: &Path, args: &[&str]) -> Result<String, String> {
    #[cfg(test)]
    let _ = CALLS.try_with(|n| n.set(n.get() + 1));
    let out = command(dir, wsl::of(dir).as_ref(), args)
        .output()
        .await
        .map_err(|e| format!("git을 실행하지 못했다: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
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
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
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
    process_tree::new_session(cmd.as_std_mut());
    cmd
}

#[cfg(test)]
pub fn test_run(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(cmd: &Command) -> Vec<String> {
        let cmd = cmd.as_std();
        std::iter::once(cmd.get_program())
            .chain(cmd.get_args())
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn wsl_repo_runs_the_distro_git() {
        let repo = r"\\wsl.localhost\Ubuntu\home\ktadev\src\loom";
        let wt = "/home/ktadev/loom/workspaces/loom/찾기사이드바";
        let cmd = command(
            Path::new(repo),
            wsl::location(repo).as_ref(),
            &["worktree", "remove", "--force", wt],
        );
        assert_eq!(
            line(&cmd),
            [
                "wsl.exe",
                "-d",
                "Ubuntu",
                "-e",
                "git",
                "-C",
                "/home/ktadev/src/loom",
                "worktree",
                "remove",
                "--force",
                wt
            ]
        );
    }

    #[test]
    fn other_repo_runs_git_here() {
        let dir = Path::new("/home/ktadev/src/loom");
        assert_eq!(
            line(&command(dir, None, &["status"])),
            [
                "git",
                "-c",
                "safe.directory=/home/ktadev/src/loom",
                "-C",
                "/home/ktadev/src/loom",
                "status"
            ]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn clone_fails_instead_of_waiting_for_an_ssh_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let ssh = dir.path().join("fake-ssh");
        crate::fake_exe::write_executable(
            &ssh,
            "#!/bin/sh\nif : </dev/tty 2>/dev/null; then echo asked >&2; sleep 30; fi\nexit 255\n",
        );
        let mut cmd = clone_command("ssh://git@example.invalid/app.git", &dir.path().join("app"));
        cmd.env("GIT_SSH_COMMAND", &ssh);
        let out = tokio::time::timeout(std::time::Duration::from_secs(10), cmd.output())
            .await
            .expect("clone waited for a prompt")
            .unwrap();
        assert!(!out.status.success());
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stderr.contains("asked"), "{stderr}");
        assert!(!dir.path().join("app").exists());
    }
}
