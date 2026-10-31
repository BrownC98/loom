use std::fmt;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

const ALIVE_EVERY: Duration = Duration::from_secs(15);
const CONNECT_WAIT: Duration = Duration::from_secs(10);
const PROBE_WAIT: Duration = Duration::from_secs(30);
const INSTALL_WAIT: Duration = Duration::from_secs(600);
const SSH_FAILED: i32 = 255;
const HOST_KEY_FAILED: &str = "Host key verification failed";
const AUTH_FAILED: &str = "Permission denied";
const NEED: &str = "need";
pub const REMOTE_DIR: &str = ".loom/remote";
pub const LOGIN_PATH: &str = r#"sep=$(printf '\036')
p=$("${SHELL:-/bin/sh}" -lic 'printf "\036%s\036" "$PATH"' </dev/null 2>/dev/null)
case $p in *"$sep"*"$sep"*) p=${p#*"$sep"}; PATH=${p%%"$sep"*} ;; esac
"#;
const CURRENT: &str = "loom";
const MAC_CPU: &str = "arm64";
const NO_LINUX: &str = "this daemon has no Linux loom to copy";
const MAX_TEXT: u64 = 64 * 1024;
pub const SSH_DIR: &str = if cfg!(windows) {
    r"%USERPROFILE%\.ssh"
} else {
    "~/.ssh"
};

#[derive(Clone, Debug, PartialEq)]
pub enum Failure {
    UnknownHostKey { host: String, folder: &'static str },
    AuthFailed,
    Unreachable(String),
    UnsupportedMac { cpu: String },
    VersionMismatch,
    Failed(String),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::UnknownHostKey { host, folder } => write!(
                f,
                "this PC does not know the host key of {host}. Run `ssh {host}` once in a terminal and accept the key. ssh keeps it in {folder}"
            ),
            Failure::AuthFailed => f.write_str(
                "the SSH login failed. Put the key of this PC in authorized_keys on the host. loom does not type passwords",
            ),
            Failure::Unreachable(cause) => write!(f, "the host is unreachable: {cause}"),
            Failure::UnsupportedMac { cpu } => write!(
                f,
                "loom runs only on Apple Silicon Macs. This Mac has {cpu}"
            ),
            Failure::VersionMismatch => f.write_str(
                "the macOS loom release speaks a different distro host version. Make a new release, then connect again",
            ),
            Failure::Failed(cause) => f.write_str(cause),
        }
    }
}

pub struct Release {
    pub file: String,
    pub url: String,
}

pub type MacRelease = fn() -> Result<Release, String>;

pub struct Outer {
    pub argv: Vec<String>,
    pub remote: Remote,
    pub released: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Remote {
    pub program: String,
    pub home: String,
    pub system: System,
}

impl Remote {
    pub fn current(&self) -> String {
        format!("{}/{REMOTE_DIR}/{CURRENT}", self.home)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum System {
    Linux,
    Mac,
    Windows { distro: String },
}

enum Probed {
    Linux,
    Mac { cpu: String },
    Windows,
}

impl Probed {
    fn wrap(&self, command: &str) -> String {
        match self {
            Probed::Linux | Probed::Mac { .. } => command.to_owned(),
            Probed::Windows => format!("wsl.exe --cd ~ --exec {command}"),
        }
    }

    fn distro_line(&self) -> &'static str {
        match self {
            Probed::Linux | Probed::Mac { .. } => "",
            Probed::Windows => "printenv WSL_DISTRO_NAME;",
        }
    }
}

struct Ran {
    code: Option<i32>,
    lines: Vec<String>,
    stderr: String,
}

impl Ran {
    fn last(&self) -> &str {
        self.line_back(0)
    }

    fn line_back(&self, skip: usize) -> &str {
        self.lines
            .iter()
            .rev()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty())
            .nth(skip)
            .unwrap_or_default()
    }

    fn text(&self) -> String {
        [self.lines.join(" "), self.stderr.clone()]
            .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
            .into_iter()
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

pub fn prepare(
    host: &str,
    linux: Option<(&Path, &str)>,
    mac: MacRelease,
) -> Result<Outer, Failure> {
    let probed = probe(host)?;
    let (file, fetch, input) = match &probed {
        Probed::Linux | Probed::Windows => {
            let (path, file) = linux.ok_or_else(|| Failure::Failed(NO_LINUX.into()))?;
            let fetch = format!("echo {NEED} && cat > {file}.partial");
            (file.to_owned(), fetch, Some(path))
        }
        Probed::Mac { cpu } if cpu == MAC_CPU => {
            let release = mac().map_err(Failure::Failed)?;
            let fetch = format!("curl -fsSL -o {}.partial {}", release.file, release.url);
            (release.file, fetch, None)
        }
        Probed::Mac { cpu } => return Err(Failure::UnsupportedMac { cpu: cpu.clone() }),
    };
    let script = format!(
        "mkdir -p {REMOTE_DIR} && cd {REMOTE_DIR} && {{ [ -x {file} ] || {{ {fetch} && chmod 755 {file}.partial && mv {file}.partial {file}; }}; }} && ln -sf {file} {CURRENT}.new && mv -f {CURRENT}.new {CURRENT} && {{ ls | grep ^loom- | grep -vxF {file} | xargs rm -f; {} pwd; }}",
        probed.distro_line()
    );
    let ran = call(
        host,
        &probed.wrap(&format!("sh -c \"{script}\"")),
        input,
        INSTALL_WAIT,
    )?;
    let home = match ran.code {
        Some(0) => ran.last().strip_suffix(&format!("/{REMOTE_DIR}")),
        Some(SSH_FAILED) => return Err(refused(host, &ran.stderr)),
        _ => None,
    };
    let home = home
        .filter(|home| home.starts_with('/'))
        .ok_or_else(|| Failure::Failed(format!("{host}: install: {}", ran.text())))?
        .to_owned();
    let system = match probed {
        Probed::Linux => System::Linux,
        Probed::Mac { .. } => System::Mac,
        Probed::Windows => System::Windows {
            distro: ran.line_back(1).to_owned(),
        },
    };
    Ok(Outer {
        argv: command(
            host,
            &probed.wrap(&format!("{REMOTE_DIR}/{file} distro-host --inner")),
        ),
        released: system == System::Mac,
        remote: Remote {
            program: format!("{home}/{REMOTE_DIR}/{file}"),
            home,
            system,
        },
    })
}

fn probe(host: &str) -> Result<Probed, Failure> {
    let ran = call(host, "uname -sm", None, PROBE_WAIT)?;
    let mut words = ran.last().split_whitespace();
    Ok(match (ran.code, words.next(), words.next()) {
        (Some(SSH_FAILED), ..) => return Err(refused(host, &ran.stderr)),
        (None, ..) => return Err(Failure::Failed(format!("{host}: ssh ended by a signal"))),
        (Some(0), Some("Linux"), _) => Probed::Linux,
        (Some(0), Some("Darwin"), Some(cpu)) => Probed::Mac {
            cpu: cpu.to_owned(),
        },
        (Some(_), ..) => Probed::Windows,
    })
}

fn refused(host: &str, stderr: &str) -> Failure {
    match stderr.trim() {
        text if text.contains(HOST_KEY_FAILED) => Failure::UnknownHostKey {
            host: host.to_owned(),
            folder: SSH_DIR,
        },
        text if text.contains(AUTH_FAILED) => Failure::AuthFailed,
        text => Failure::Unreachable(text.split_whitespace().collect::<Vec<_>>().join(" ")),
    }
}

fn program() -> String {
    crate::search_path::find("ssh").map_or_else(|| "ssh".into(), |p| p.display().to_string())
}

fn command(host: &str, remote: &str) -> Vec<String> {
    let alive = format!("ServerAliveInterval={}", ALIVE_EVERY.as_secs());
    let connect = format!("ConnectTimeout={}", CONNECT_WAIT.as_secs());
    let options = [
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        &alive,
        "-o",
        &connect,
        "--",
        host,
        remote,
    ];
    std::iter::once(program())
        .chain(options.map(str::to_owned))
        .collect()
}

fn call(host: &str, remote: &str, input: Option<&Path>, limit: Duration) -> Result<Ran, Failure> {
    let argv = command(host, remote);
    let ssh = &argv[0];
    let mut cmd = Command::new(ssh);
    cmd.args(&argv[1..])
        .stdin(match input {
            Some(_) => Stdio::piped(),
            None => Stdio::null(),
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::timed::own_session(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| Failure::Failed(format!("{ssh}: {e}")))?;
    let mut stdin = child.stdin.take();
    let stdout = child.stdout.take().expect("stdout을 piped로 열었다");
    let mut stderr = child.stderr.take().expect("stderr를 piped로 열었다");
    let input: Option<PathBuf> = input.map(Path::to_owned);
    let (answered, answer) = mpsc::channel();
    std::thread::spawn(move || {
        let mut lines = Vec::new();
        let mut sent = Ok(());
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line.trim() == NEED
                && let (Some(mut to), Some(from)) = (stdin.take(), &input)
            {
                sent = std::fs::File::open(from)
                    .and_then(|mut from| io::copy(&mut from, &mut to))
                    .map(drop);
                continue;
            }
            lines.push(line);
        }
        let _ = answered.send((lines, sent));
    });
    let (said, errors) = mpsc::channel();
    std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = (&mut stderr).take(MAX_TEXT).read_to_end(&mut text);
        let _ = said.send(String::from_utf8_lossy(&text).into_owned());
        let _ = io::copy(&mut stderr, &mut io::sink());
    });
    let deadline = Instant::now() + limit;
    let no_answer = || {
        Failure::Unreachable(format!(
            "{host} did not answer in {} seconds",
            limit.as_secs()
        ))
    };
    let Ok((lines, sent)) = answer.recv_timeout(limit) else {
        crate::timed::stop(child);
        return Err(no_answer());
    };
    let status = crate::timed::wait_until(child, deadline).ok_or_else(no_answer)?;
    let stderr = errors
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .unwrap_or_default();
    sent.map_err(|e| Failure::Failed(format!("{host}: copy loom: {e}")))?;
    Ok(Ran {
        code: status.code(),
        lines,
        stderr,
    })
}
