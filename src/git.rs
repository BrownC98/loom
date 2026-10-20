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
