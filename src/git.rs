//! 기반: `git` CLI를 부른다. 근거: docs/design.md 「기술 스택」.

use std::path::Path;

use tokio::process::Command;

/// `dir`에서 `git <args>`를 돌리고 표준 출력을 돌려준다. 실패하면 git의 오류 글이다.
pub async fn run(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
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
/// SSH 호스트 키 확인은 그대로다. 데몬은 제어 터미널 없이 돈다(`scripts/host.sh`의 `setsid`). 그래서 모르는 호스트면 실패한다.
pub async fn clone(url: &str, target: &Path) -> Result<(), String> {
    let out = Command::new("git")
        .args(["clone", "--", url])
        .arg(target)
        .env("GIT_TERMINAL_PROMPT", "0")
        // 빈 값이면 git이 질문 프로그램(`core.askPass`, `SSH_ASKPASS`)을 부르지 않는다.
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("GCM_INTERACTIVE", "never")
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| format!("git을 실행하지 못했다: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    Ok(())
}
