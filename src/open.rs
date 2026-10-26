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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_target_uses_backslashes() {
        assert_eq!(
            backslashes(Path::new("G:/a b/한글.html")),
            OsString::from(r"G:\a b\한글.html")
        );
        assert_eq!(
            backslashes(Path::new("//wsl.localhost/Ubuntu/home/a b.html")),
            OsString::from(r"\\wsl.localhost\Ubuntu\home\a b.html")
        );
    }

    #[cfg(windows)]
    #[test]
    fn opener_receives_backslash_path() {
        let dir = std::env::temp_dir().join(format!("loom-open-{}", std::process::id()));
        let sub = dir.join("a b");
        std::fs::create_dir_all(&sub).unwrap();
        let file = sub.join("한글.html");
        std::fs::write(&file, "x").unwrap();
        let log = dir.join("arg.txt");
        let rec = dir.join("rec.cmd");
        std::fs::write(
            &rec,
            format!("@chcp 65001 >nul\r\n@echo %~1> \"{}\"\r\n", log.display()),
        )
        .unwrap();
        unsafe { std::env::set_var(OPENER_ENV, &rec) };
        open(Path::new(&file.to_string_lossy().replace('\\', "/"))).unwrap();
        let want = file.to_string_lossy().into_owned();
        let got = (0..50)
            .find_map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(100));
                std::fs::read_to_string(&log)
                    .ok()
                    .filter(|s| s.trim_end().ends_with("html"))
            })
            .unwrap_or_default();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(got.trim_end(), want);
    }

    fn vars(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn only_the_humans_loom_gets_the_system_opener() {
        let host = Host::Linux;
        let denied =
            |r: io::Result<OsString>| r.unwrap_err().kind() == io::ErrorKind::PermissionDenied;
        assert_eq!(opener(host, vars(&[]), || true).unwrap(), "xdg-open");
        assert!(denied(opener(host, vars(&[]), || false)));
        assert!(denied(opener(host, vars(&[(NODE_TOKEN_ENV, "t")]), || {
            true
        })));
        assert_eq!(
            opener(
                host,
                vars(&[(OPENER_ENV, "rec"), (NODE_TOKEN_ENV, "t")]),
                || false
            )
            .unwrap(),
            "rec"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_and_node_looms_run_no_system_opener() {
        const CHILD: &str = "LOOM_OPEN_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let err = open_url("http://127.0.0.1:1/").unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let log = dir.path().join("system.log");
        for name in ["xdg-open", "explorer.exe", "open"] {
            let fake = bin.join(name);
            crate::fake_exe::write_executable(
                &fake,
                &format!("#!/bin/sh\necho {name} >> '{}'\n", log.display()),
            );
        }
        let path = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .unwrap();
        let test_home = dir.path().join("home");
        std::fs::create_dir(&test_home).unwrap();
        let real_home = std::env::var_os("HOME").unwrap_or_default();
        let cases: [(&std::ffi::OsStr, Option<&str>); 2] = [
            (test_home.as_os_str(), None),
            (real_home.as_os_str(), Some("t1")),
        ];
        for (home, token) in cases {
            let mut cmd = Command::new(std::env::current_exe().unwrap());
            cmd.args([
                "--exact",
                "open::tests::test_and_node_looms_run_no_system_opener",
            ])
            .env(CHILD, "1")
            .env("HOME", home)
            .env("PATH", &path)
            .env_remove(OPENER_ENV)
            .env_remove(NODE_TOKEN_ENV);
            if let Some(t) = token {
                cmd.env(NODE_TOKEN_ENV, t);
            }
            let out = cmd.output().unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stdout)
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(
            !log.exists(),
            "{}",
            std::fs::read_to_string(&log).unwrap_or_default()
        );
    }

    #[test]
    fn each_host_has_its_own_opener() {
        assert_eq!(Host::Wsl.opener(), "explorer.exe");
        assert_eq!(Host::Mac.opener(), "open");
        assert_eq!(Host::Linux.opener(), "xdg-open");
        assert_eq!(Host::Windows.opener(), "explorer");
    }
}
