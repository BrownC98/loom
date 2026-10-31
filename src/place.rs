use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::distro::{self, Command, Distros, Entry, Host, Ran, SSH_NOT_CONNECTED};
use crate::wsl::{self, Inside, WslPath};

const SSH_SCHEME: &str = "ssh://";

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

    pub async fn run(&self, host: &str, command: Command) -> Result<Ran, String> {
        let hosts = Arc::clone(
            self.hosts
                .ok_or_else(|| format!("{host}: {SSH_NOT_CONNECTED}"))?,
        );
        let host = Host::Ssh(host.to_owned());
        tokio::task::spawn_blocking(move || hosts.run(&host, command))
            .await
            .map_err(|e| e.to_string())?
    }

    pub fn of(inside: Option<&'a Inside>) -> Self {
        Self::new(inside.map(|inside| inside.distro.hosts()))
    }

    pub fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        match of(path) {
            Machine::Pc | Machine::Wsl(_) => std::fs::read(path),
            Machine::Ssh(at) => self.hosts(&at)?.read(&Host::Ssh(at.host), &at.path),
        }
    }

    pub fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        match of(path) {
            Machine::Pc | Machine::Wsl(_) => distro::replace(path, bytes),
            Machine::Ssh(at) => self.hosts(&at)?.write(&Host::Ssh(at.host), &at.path, bytes),
        }
    }

    pub fn list(&self, dir: &Path) -> io::Result<Vec<Entry>> {
        match of(dir) {
            Machine::Pc | Machine::Wsl(_) => distro::list(dir),
            Machine::Ssh(at) => self.hosts(&at)?.list(&Host::Ssh(at.host), &at.path),
        }
    }

    fn hosts(&self, at: &SshPath) -> io::Result<&'a Arc<Distros>> {
        self.hosts
            .ok_or_else(|| io::Error::other(format!("{}: {SSH_NOT_CONNECTED}", at.host)))
    }
}
