use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::distro::{self, Command, Distros, Entry, Host, Ran, SSH_NOT_CONNECTED};
use crate::wsl::{self, Inside, WslPath};

const SSH_SCHEME: &str = "ssh://";
pub const RUN_WAIT: Duration = Duration::from_secs(30);
const EXISTED: i32 = 3;
const MISSING: i32 = 4;
const MIRRORED: &str = ".loom-mirror";
const MIRROR_DIGEST: usize = 8;

#[derive(Clone, Debug, PartialEq)]
pub enum Machine {
    Pc,
    Wsl(WslPath),
    Ssh(SshPath),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SshPath {
    pub host: String,
    pub path: String,
}

impl SshPath {
    pub fn url(&self, inner: &str) -> String {
        format!("{SSH_SCHEME}{}{}", self.host, inner.trim_end_matches('/'))
    }
}

pub fn of(path: &Path) -> Machine {
    path.to_str().map_or(Machine::Pc, parse)
}

pub fn parse(path: &str) -> Machine {
    if let Some(at) = ssh(path) {
        return Machine::Ssh(at);
    }
    match wsl::location(path).filter(|_| cfg!(windows)) {
        Some(at) => Machine::Wsl(at),
        None => Machine::Pc,
    }
}

fn ssh(path: &str) -> Option<SshPath> {
    let path = path.replace('\\', "/");
    let rest = path.strip_prefix(SSH_SCHEME)?;
    let (host, inner) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    if host.is_empty() {
        return None;
    }
    Some(SshPath {
        host: host.to_owned(),
        path: match inner {
            "" => "/".into(),
            inner => inner.to_owned(),
        },
    })
}

impl Machine {
    pub fn host(&self) -> Option<Host> {
        match self {
            Machine::Pc => None,
            Machine::Wsl(at) => Some(Host::Distro(at.distro.clone())),
            Machine::Ssh(at) => Some(Host::Ssh(at.host.clone())),
        }
    }

    pub fn same(&self, other: &Machine) -> bool {
        let folded = |name: &str| name.to_lowercase();
        match (self, other) {
            (Machine::Pc, Machine::Pc) => true,
            (Machine::Wsl(a), Machine::Wsl(b)) => folded(&a.distro) == folded(&b.distro),
            (Machine::Ssh(a), Machine::Ssh(b)) => folded(&a.host) == folded(&b.host),
            (Machine::Pc | Machine::Wsl(_) | Machine::Ssh(_), _) => false,
        }
    }

    pub fn local(&self, inner: &str) -> String {
        let linux = inner.starts_with('/') && !inner.starts_with("//");
        match self {
            Machine::Wsl(at) if linux => at.unc(inner),
            Machine::Ssh(at) if linux => at.url(inner),
            Machine::Pc | Machine::Wsl(_) | Machine::Ssh(_) => inner.to_owned(),
        }
    }
}

pub fn inner(path: &str) -> String {
    match parse(path) {
        Machine::Pc => path.to_owned(),
        Machine::Wsl(at) => at.path,
        Machine::Ssh(at) => at.path,
    }
}

pub fn from_node(path: PathBuf, folder: &Path) -> PathBuf {
    match path.to_str() {
        Some(text) => of(folder).local(text).into(),
        None => path,
    }
}

pub fn absolute(path: &Path) -> bool {
    match of(path) {
        Machine::Pc | Machine::Wsl(_) => path.is_absolute(),
        Machine::Ssh(_) => true,
    }
}

pub fn parent(path: &Path) -> Option<String> {
    match of(path) {
        Machine::Pc | Machine::Wsl(_) => path.parent().map(|p| p.to_string_lossy().into_owned()),
        Machine::Ssh(at) => {
            let up = Path::new(&at.path).parent()?;
            Some(at.url(&up.to_string_lossy()))
        }
    }
}

#[derive(Clone, Copy)]
pub struct Files<'a> {
    hosts: Option<&'a Arc<Distros>>,
}

impl<'a> Files<'a> {
    pub fn new(hosts: Option<&'a Arc<Distros>>) -> Self {
        Self { hosts }
    }

    pub async fn run(&self, host: &str, command: Command, limit: Duration) -> Result<Ran, String> {
        let hosts = Arc::clone(
            self.hosts
                .ok_or_else(|| format!("{host}: {SSH_NOT_CONNECTED}"))?,
        );
        let host = Host::Ssh(host.to_owned());
        tokio::task::spawn_blocking(move || hosts.run_within(&host, command, limit))
            .await
            .map_err(|e| e.to_string())?
    }

    pub fn of(inside: Option<&'a Inside>) -> Self {
        Self::new(inside.map(|inside| inside.distro.hosts()))
    }

    pub fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        match of(path) {
            Machine::Pc | Machine::Wsl(_) => std::fs::read(path),
            Machine::Ssh(at) => {
                let hosts = self.hosts(&at)?;
                off_worker(|| hosts.read(&Host::Ssh(at.host), &at.path))
            }
        }
    }

    pub fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        match of(path) {
            Machine::Pc | Machine::Wsl(_) => distro::replace(path, bytes),
            Machine::Ssh(at) => {
                let hosts = self.hosts(&at)?;
                off_worker(|| hosts.write(&Host::Ssh(at.host), &at.path, bytes))
            }
        }
    }

    pub fn list(&self, dir: &Path) -> io::Result<Vec<Entry>> {
        match of(dir) {
            Machine::Pc | Machine::Wsl(_) => distro::list(dir),
            Machine::Ssh(at) => {
                let hosts = self.hosts(&at)?;
                off_worker(|| hosts.list(&Host::Ssh(at.host), &at.path))
            }
        }
    }

    pub fn create_dir_all(&self, dir: &Path) -> io::Result<()> {
        match of(dir) {
            Machine::Pc | Machine::Wsl(_) => std::fs::create_dir_all(dir),
            Machine::Ssh(at) => self.sh(&at, r#"mkdir -p -- "$1""#, &[]),
        }
    }

    pub fn create_dir(&self, dir: &Path) -> io::Result<()> {
        match of(dir) {
            Machine::Pc | Machine::Wsl(_) => std::fs::create_dir(dir),
            Machine::Ssh(at) => self.sh(
                &at,
                &format!(
                    r#"mkdir -- "$1" 2>/dev/null && exit 0; [ -e "$1" ] && exit {EXISTED}; mkdir -- "$1""#
                ),
                &[],
            ),
        }
    }

    pub fn create_new(&self, path: &Path, text: &str) -> io::Result<()> {
        match of(path) {
            Machine::Pc | Machine::Wsl(_) => std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?
                .write_all(text.as_bytes()),
            Machine::Ssh(at) => {
                self.sh(
                    &at,
                    &format!(
                        r#"(set -C; : > "$1") 2>/dev/null && exit 0; [ -e "$1" ] && exit {EXISTED}; (set -C; : > "$1")"#
                    ),
                    &[],
                )?;
                match text.is_empty() {
                    true => Ok(()),
                    false => self.write(path, text.as_bytes()),
                }
            }
        }
    }

    pub fn is_file(&self, path: &Path) -> io::Result<bool> {
        match of(path) {
            Machine::Pc | Machine::Wsl(_) => std::fs::metadata(path).map(|meta| meta.is_file()),
            Machine::Ssh(at) => self
                .sh(&at, &format!(r#"[ -f "$1" ] || exit {MISSING}"#), &[])
                .map(|()| true),
        }
        .or_else(|e| match e.kind() {
            io::ErrorKind::NotFound => Ok(false),
            _ => Err(e),
        })
    }

    pub fn rewrite(&self, path: &Path, text: &str) -> io::Result<()> {
        match of(path) {
            Machine::Pc | Machine::Wsl(_) => std::fs::write(path, text),
            Machine::Ssh(at) => self.sh(&at, r#"printf %s "$2" > "$1""#, &[text]),
        }
    }

    pub fn append(&self, path: &Path, text: &str) -> io::Result<()> {
        match of(path) {
            Machine::Pc | Machine::Wsl(_) => std::fs::OpenOptions::new()
                .append(true)
                .open(path)?
                .write_all(text.as_bytes()),
            Machine::Ssh(at) => self.sh(
                &at,
                &format!(r#"[ -e "$1" ] || exit {MISSING}; printf %s "$2" >> "$1""#),
                &[text],
            ),
        }
    }

    pub fn remove(&self, path: &Path) -> io::Result<()> {
        match of(path) {
            Machine::Pc | Machine::Wsl(_) => std::fs::remove_file(path),
            Machine::Ssh(at) => self.sh(
                &at,
                &format!(r#"[ -e "$1" ] || [ -L "$1" ] || exit {MISSING}; rm -f -- "$1""#),
                &[],
            ),
        }
    }

    pub fn remove_dir_all(&self, dir: &Path) -> io::Result<()> {
        match of(dir) {
            Machine::Pc | Machine::Wsl(_) => std::fs::remove_dir_all(dir),
            Machine::Ssh(at) => self.sh(
                &at,
                &format!(r#"[ -e "$1" ] || exit {MISSING}; rm -rf -- "$1""#),
                &[],
            ),
        }
    }

    pub fn mirror(&self, from: &Path, to: &Path) -> io::Result<()> {
        let mut files = Vec::new();
        local_files(from, "", &mut files)?;
        files.sort();
        let digest = |bytes: &[u8]| -> String {
            use sha2::Digest;
            sha2::Sha256::digest(bytes)[..MIRROR_DIGEST]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        };
        let lines: Vec<String> = files
            .iter()
            .map(|(name, bytes)| format!("{} {name}", digest(bytes)))
            .collect();
        let manifest = to.join(MIRRORED);
        let old = match self.read(&manifest) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        let had: std::collections::HashSet<&str> = old.lines().collect();
        for ((name, bytes), line) in files.iter().zip(&lines) {
            if !had.contains(line.as_str()) {
                self.write(&to.join(name), bytes)?;
            }
        }
        let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
        match text == old {
            true => Ok(()),
            false => self.write(&manifest, text.as_bytes()),
        }
    }

    fn sh(&self, at: &SshPath, script: &str, args: &[&str]) -> io::Result<()> {
        let argv = ["sh", "-c", script, "sh", &at.path]
            .into_iter()
            .chain(args.iter().copied())
            .map(str::to_owned)
            .collect();
        let command = distro::Command {
            argv,
            cwd: "/".into(),
            env: Vec::new(),
        };
        let hosts = self.hosts(at)?;
        let ran = off_worker(|| hosts.run_within(&Host::Ssh(at.host.clone()), command, RUN_WAIT))
            .map_err(io::Error::other)?;
        match ran.code {
            Some(0) => Ok(()),
            Some(EXISTED) => Err(io::ErrorKind::AlreadyExists.into()),
            Some(MISSING) => Err(io::ErrorKind::NotFound.into()),
            _ => Err(io::Error::other(format!(
                "{}: {}",
                at.url(&at.path),
                ran.stderr.trim()
            ))),
        }
    }

    fn hosts(&self, at: &SshPath) -> io::Result<&'a Arc<Distros>> {
        self.hosts
            .ok_or_else(|| io::Error::other(format!("{}: {SSH_NOT_CONNECTED}", at.host)))
    }
}

fn off_worker<T>(work: impl FnOnce() -> T) -> T {
    use tokio::runtime::{Handle, RuntimeFlavor};
    match Handle::try_current().map(|handle| handle.runtime_flavor()) {
        Ok(RuntimeFlavor::MultiThread) => tokio::task::block_in_place(work),
        _ => work(),
    }
}

fn local_files(dir: &Path, prefix: &str, out: &mut Vec<(String, Vec<u8>)>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
        match entry.file_type()?.is_dir() {
            true => local_files(&entry.path(), &format!("{name}/"), out)?,
            false => out.push((name, std::fs::read(entry.path())?)),
        }
    }
    Ok(())
}
