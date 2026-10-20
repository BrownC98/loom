//! 사람이 터미널에서 누른 파일 링크(`file://`)를 그 기계의 기본 앱으로 연다.
//! 브라우저 페이지는 `file://`을 열지 못하므로 데몬이 대신 연다. WSL이면 Windows 쪽 앱으로 연다.
//! 근거: docs/design.md 「저장」의 터미널 링크 줄.

use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

/// 있는 파일이나 폴더만 연다. 앱이 뜨기를 기다리지 않는다.
pub fn open(path: &Path) -> io::Result<()> {
    if !path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            path.display().to_string(),
        ));
    }
    let mut child = opener(path)?
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // explorer.exe는 연 뒤에도 종료 코드 1을 돌려준다. 그래서 종료 코드를 보지 않고 거두기만 한다.
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(windows)]
fn opener(path: &Path) -> io::Result<Command> {
    let mut cmd = Command::new("explorer");
    cmd.arg(path);
    Ok(cmd)
}

#[cfg(target_os = "macos")]
fn opener(path: &Path) -> io::Result<Command> {
    let mut cmd = Command::new("open");
    cmd.arg(path);
    Ok(cmd)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn opener(path: &Path) -> io::Result<Command> {
    // WSL은 이 파일로 Windows 실행 파일을 부른다. 있으면 Windows 경로로 바꿔 explorer.exe에 넘긴다.
    if !Path::new("/proc/sys/fs/binfmt_misc/WSLInterop").exists() {
        let mut cmd = Command::new("xdg-open");
        cmd.arg(path);
        return Ok(cmd);
    }
    let out = Command::new("wslpath").arg("-w").arg(path).output()?;
    if !out.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ));
    }
    let mut cmd = Command::new("explorer.exe");
    cmd.arg(String::from_utf8_lossy(&out.stdout).trim_end());
    Ok(cmd)
}
