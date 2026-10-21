//! 기반: `git` CLI를 부른다. 근거: docs/design.md 「기술 스택」.

use std::path::Path;

use tokio::process::Command;

/// `dir`에서 `git <args>`를 돌리고 표준 출력을 돌려준다. 실패하면 git의 오류 글이다.
pub async fn run(dir: &Path, args: &[&str]) -> Result<String, String> {
    // Windows git은 소유자가 다른 폴더(`\\wsl.localhost\…`)를 "dubious ownership"으로 거부한다.
    // loom은 사람이 고른 저장소와 그 워크트리에서만 git을 부른다. 그 폴더만 믿는다.
    let mut safe = std::ffi::OsString::from("safe.directory=");
    safe.push(dir);
    let out = Command::new("git")
        .arg("-c")
        .arg(safe)
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .await
        .map_err(|e| format!("git을 실행하지 못했다: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
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
    detach_terminal(&mut cmd);
    cmd
}

/// ssh는 비밀번호·암호문·호스트 키 질문을 제어 터미널(`/dev/tty`)에서 읽는다. 새 세션에는 제어 터미널이 없다.
#[cfg(unix)]
fn detach_terminal(cmd: &mut Command) {
    // SAFETY: `setsid`는 fork 뒤 exec 전에 불러도 되는 함수(async-signal-safe)다.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// 콘솔 없이 띄운다. ssh가 질문을 읽을 콘솔이 없다.
#[cfg(windows)]
fn detach_terminal(cmd: &mut Command) {
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    cmd.creation_flags(DETACHED_PROCESS);
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// 가짜 ssh가 제어 터미널을 열 수 있으면 질문을 기다린다(30초). 열지 못하면 바로 실패한다.
    /// 제어 터미널이 있는 셸에서 돌려야 뜻이 있다(`script -qc 'cargo test git::'`).
    #[tokio::test]
    async fn clone_fails_instead_of_waiting_for_an_ssh_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let ssh = dir.path().join("fake-ssh");
        std::fs::write(
            &ssh,
            "#!/bin/sh\nif : </dev/tty 2>/dev/null; then echo asked >&2; sleep 30; fi\nexit 255\n",
        )
        .unwrap();
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
