//! 파일·폴더·주소를 그 기계의 기본 앱으로 연다. WSL이면 Windows 쪽 앱으로 연다.
//! 파일: 사람이 터미널에서 누른 링크(`file://`)와 `loom show`. 브라우저 페이지는 `file://`을 열지 못하므로 데몬이 대신 연다.
//! 주소: `loom open`이 화면 주소를 연다.
//! 근거: docs/design.md 「저장」의 터미널 링크 줄, 「배포」의 `loom open` 줄.

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

/// 이 변수가 있으면 기본 앱 대신 이 프로그램에 대상을 넘긴다. 시험이 사람의 브라우저를 열지 않게 기록기를 넣는다.
const OPENER_ENV: &str = "LOOM_BROWSER";

/// 있는 파일이나 폴더만 연다. 앱이 뜨기를 기다리지 않는다.
pub fn open(path: &Path) -> io::Result<()> {
    if !path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            path.display().to_string(),
        ));
    }
    let host = Host::current();
    let target = match host {
        Host::Wsl => windows_path(path)?,
        _ => path.as_os_str().to_owned(),
    };
    launch(host, target)
}

/// 주소(`http://…`)를 기본 브라우저로 연다. WSL은 Windows 기본 브라우저다.
pub fn open_url(url: &str) -> io::Result<()> {
    launch(Host::current(), url.into())
}

fn launch(host: Host, target: OsString) -> io::Result<()> {
    let mut cmd = match std::env::var_os(OPENER_ENV) {
        Some(program) => Command::new(program),
        None => Command::new(host.opener()),
    };
    let mut child = cmd
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // explorer.exe는 연 뒤에도 종료 코드 1을 돌려준다. 그래서 종료 코드를 보지 않고 거두기만 한다.
    // 짧게 사는 `loom open`은 거두기 전에 끝날 수 있다. 그래도 앱은 따로 산다.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// 기본 앱을 여는 방법이 다른 실행 환경.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Host {
    Windows,
    Mac,
    /// Linux 실행 파일이지만 Windows 쪽 앱으로 연다.
    Wsl,
    Linux,
}

impl Host {
    fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Mac
        } else if is_wsl() {
            Self::Wsl
        } else {
            Self::Linux
        }
    }

    /// 대상 하나를 인자로 받는 여는 프로그램.
    fn opener(self) -> &'static str {
        match self {
            Self::Windows => "explorer",
            Self::Mac => "open",
            Self::Wsl => "explorer.exe",
            Self::Linux => "xdg-open",
        }
    }
}

/// WSL은 `WSLInterop` 파일로 Windows 실행 파일을 부른다. 새 WSL은 이름이 `WSLInterop-late`다.
/// `wsl.exe`가 띄운 프로세스에는 `WSL_DISTRO_NAME`도 있다.
fn is_wsl() -> bool {
    std::env::var_os("WSL_DISTRO_NAME").is_some()
        || ["WSLInterop", "WSLInterop-late"]
            .iter()
            .any(|name| Path::new("/proc/sys/fs/binfmt_misc").join(name).exists())
}

fn windows_path(path: &Path) -> io::Result<OsString> {
    let out = Command::new("wslpath").arg("-w").arg(path).output()?;
    if !out.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_host_has_its_own_opener() {
        assert_eq!(Host::Wsl.opener(), "explorer.exe");
        assert_eq!(Host::Mac.opener(), "open");
        assert_eq!(Host::Linux.opener(), "xdg-open");
        assert_eq!(Host::Windows.opener(), "explorer");
    }
}
