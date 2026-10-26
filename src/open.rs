use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

mod account;

pub const OPENER_ENV: &str = "LOOM_BROWSER";

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
        Host::Windows => backslashes(path),
        _ => path.as_os_str().to_owned(),
    };
    launch(host, target)
}

fn backslashes(path: &Path) -> OsString {
    path.to_string_lossy().replace('/', "\\").into()
}

pub fn open_url(url: &str) -> io::Result<()> {
    launch(Host::current(), url.into())
}

fn launch(host: Host, target: OsString) -> io::Result<()> {
    let program = opener(
        host,
        |key| std::env::var_os(key),
        account::is_account_folder,
    )?;
    let mut child = Command::new(program)
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

const NODE_TOKEN_ENV: &str = "LOOM_TOKEN";

fn opener(
    host: Host,
    var: impl Fn(&str) -> Option<OsString>,
    account_folder: impl FnOnce() -> bool,
) -> io::Result<OsString> {
    if let Some(recorder) = var(OPENER_ENV) {
        return Ok(recorder);
    }
    let refuse = |why: &str| {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{why}, so it does not open the system app. Set {OPENER_ENV} to a recorder."),
        ))
    };
    if var(NODE_TOKEN_ENV).is_some() {
        return refuse("this loom runs in a node (LOOM_TOKEN is set)");
    }
    if !account_folder() {
        return refuse(
            "this loom does not use the account's own data folder (a test HOME or APPDATA)",
        );
    }
    Ok(host.opener().into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Host {
    Windows,
    Mac,
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

    fn opener(self) -> &'static str {
        match self {
            Self::Windows => "explorer",
            Self::Mac => "open",
            Self::Wsl => "explorer.exe",
            Self::Linux => "xdg-open",
        }
    }
}

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
