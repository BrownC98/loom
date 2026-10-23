//! 기반: `git` CLI를 부른다. 근거: docs/design.md 「기술 스택」.

use std::path::Path;

use tokio::process::Command;

use crate::process_tree;
use crate::wsl::{self, WslPath};

/// `dir`에서 `git <args>`를 돌리고 표준 출력을 돌려준다. 실패하면 git의 오류 글이다.
pub async fn run(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = command(dir, wsl::of(dir).as_ref(), args)
        .output()
        .await
        .map_err(|e| format!("git을 실행하지 못했다: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Windows 데몬이 WSL 폴더(`\\wsl.localhost\…`)에서 부르는 git은 그 배포판의 git이다.
/// WSL git이 워크트리 파일(`.git`, `gitdir`)에 적은 Linux 경로를 Windows git은 읽지 못한다(`worktree remove`가 "does not point back"으로 실패한다).
fn command(dir: &Path, wsl: Option<&WslPath>, args: &[&str]) -> Command {
    let mut cmd = match wsl {
        Some(wsl) => {
            let mut cmd = Command::new("wsl.exe");
            // `-e`는 셸 없이 git을 부른다. 인자(브랜치 이름, 경로)를 셸이 다시 읽지 않는다.
            cmd.args(["-d", &wsl.distro, "-e", "git", "-C", &wsl.path]);
            cmd
        }
        None => {
            // Windows git은 소유자가 다른 폴더(`\\wsl.localhost\…`)를 "dubious ownership"으로 거부한다.
            // loom은 사람이 고른 저장소와 그 워크트리에서만 git을 부른다. 그 폴더만 믿는다.
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

/// `dir`의 git에 인자로 넘길 경로. WSL 저장소면 배포판 안의 Linux 경로다.
pub fn to_git_path(dir: &Path, path: &str) -> String {
    match (wsl::of(dir), wsl::location(path)) {
        (Some(_), Some(inside)) => inside.path,
        _ => path.to_owned(),
    }
}

/// `dir`의 git이 적은 경로를 데몬이 여는 경로로 바꾼다. WSL 저장소의 Linux 경로는 Windows git이 적던 UNC 글자가 된다.
pub fn from_git_path(dir: &Path, path: &str) -> String {
    match wsl::of(dir) {
        Some(wsl) if path.starts_with('/') => wsl.unc(path),
        _ => path.to_owned(),
    }
}

/// `git clone -- <url> <target>`. 데몬에는 답할 사람이 없다. 인증을 물어야 하면 창이나 터미널 질문 없이 실패한다.
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

/// 사용자의 git·SSH 설정(키, ssh-agent, `known_hosts`, `core.sshCommand`)은 그대로 쓴다. 호스트 키 확인도 그대로다.
/// 묻는 길만 막는다: 터미널(제어 터미널을 뗀다), 질문 프로그램, 창.
fn clone_command(url: &str, target: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(["clone", "--", url])
        .arg(target)
        .env("GIT_TERMINAL_PROMPT", "0")
        // 빈 값이면 git이 질문 프로그램(`core.askPass`, `SSH_ASKPASS`)을 부르지 않는다.
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS_REQUIRE", "never")
        // OpenSSH 8.4 전은 `SSH_ASKPASS_REQUIRE`를 모른다. 화면이 없으면 질문 창을 띄우지 않는다.
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env("GCM_INTERACTIVE", "never")
        .stdin(std::process::Stdio::null());
    // ssh는 비밀번호·암호문·호스트 키 질문을 제어 터미널(`/dev/tty`)이나 콘솔에서 읽는다. 새 세션에는 둘 다 없다.
    process_tree::new_session(cmd.as_std_mut());
    cmd
}

/// 시험의 git: 커밋할 이름을 넣고 기다려 돌린다. 실패하면 git의 오류 글로 시험을 멈춘다. 다듬은 표준 출력을 돌려준다.
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

    /// 가짜 ssh가 제어 터미널을 열 수 있으면 질문을 기다린다(30초). 열지 못하면 바로 실패한다.
    /// 제어 터미널이 있는 셸에서 돌려야 뜻이 있다(`script -qc 'cargo test git::'`).
    #[cfg(unix)]
    #[tokio::test]
    async fn clone_fails_instead_of_waiting_for_an_ssh_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let ssh = dir.path().join("fake-ssh");
        std::fs::write(
            &ssh,
            "#!/bin/sh\nif : </dev/tty 2>/dev/null; then echo asked >&2; sleep 30; fi\nexit 255\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
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
