use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;

use portable_pty::CommandBuilder;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::timed::Exit;

pub fn command(name: &str) -> std::process::Command {
    command_at(&crate::search_path::find(name).unwrap_or_else(|| name.into()))
}

pub fn color_terminal(cmd: &mut CommandBuilder) {
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env_remove("NO_COLOR");
    cmd.env_remove("FORCE_COLOR");
}

pub fn command_at(path: &Path) -> std::process::Command {
    let mut argv = argv(path).into_iter();
    let mut cmd = std::process::Command::new(argv.next().expect("argv는 비지 않는다"));
    cmd.args(argv);
    cmd
}

pub fn argv(path: &Path) -> Vec<OsString> {
    npm_shim(path).unwrap_or_else(|| vec![path.into()])
}

fn npm_shim(path: &Path) -> Option<Vec<OsString>> {
    if !path.extension()?.eq_ignore_ascii_case("cmd") {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let line = text.lines().rev().find(|l| l.contains("%*"))?;
    let script = line.rsplit_once("\"%dp0%\\")?.1.split('"').next()?;
    let dir = path.parent()?;
    if !line.contains("%_prog%") {
        return Some(vec![dir.join(script).into()]);
    }
    let node = Some(dir.join("node.exe"))
        .filter(|p| p.is_file())
        .or_else(|| crate::search_path::find("node"))?;
    Some(vec![node.into(), dir.join(script).into()])
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub pid: u32,
    pub birth: i64,
}

pub struct Snapshot {
    system: System,
    parents: HashMap<u32, u32>,
}

impl Snapshot {
    pub fn identity(&self, pid: u32) -> Option<Identity> {
        let started = self.system.process(Pid::from_u32(pid))?.start_time();
        Some(Identity {
            pid,
            birth: i64::try_from(started).ok()?,
        })
    }

    pub fn now() -> Self {
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        let parents = system
            .processes()
            .iter()
            .filter_map(|(pid, p)| Some((pid.as_u32(), p.parent()?.as_u32())))
            .collect();
        Self { system, parents }
    }

    pub fn descends(&self, pid: u32, root: u32) -> bool {
        child_on_path(&self.parents, root, pid).is_some()
    }

    pub fn child_on_path(&self, root: u32, pid: u32) -> Option<u32> {
        child_on_path(&self.parents, root, pid)
    }

    pub fn session_leaders_under(&self, root: u32) -> Vec<u32> {
        self.parents
            .keys()
            .filter(|&&pid| {
                child_on_path(&self.parents, root, pid).is_some()
                    && self
                        .system
                        .process(Pid::from_u32(pid))
                        .and_then(|p| p.session_id())
                        .is_some_and(|sid| sid.as_u32() == pid)
            })
            .copied()
            .collect()
    }

    pub fn command_lines(&self, root: u32) -> Vec<Vec<String>> {
        let pids: Vec<Pid> = subtree(&self.parents, root)
            .into_iter()
            .map(Pid::from_u32)
            .collect();
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&pids),
            true,
            ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
        );
        pids.iter()
            .filter_map(|pid| system.process(*pid))
            .map(|p| {
                p.cmd()
                    .iter()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect()
            })
            .collect()
    }

    pub fn kill_trees(&self, roots: impl IntoIterator<Item = u32>) -> usize {
        roots
            .into_iter()
            .flat_map(|root| subtree(&self.parents, root))
            .filter(|&p| {
                self.system
                    .process(Pid::from_u32(p))
                    .is_some_and(|p| p.kill())
            })
            .count()
    }

    pub fn end_trees(&self, roots: Vec<u32>) -> Result<(), String> {
        let watched: Vec<Result<Option<Exit>, String>> =
            roots.iter().map(|&pid| Exit::watch(pid)).collect();
        self.kill_trees(roots);
        let deadline = std::time::Instant::now() + END_WAIT;
        watched.into_iter().try_for_each(|watch| match watch? {
            Some(exit) => exit.wait(deadline),
            None => Ok(()),
        })
    }
}

const END_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

pub fn end_if_runs(pid: u32, arg: &str) {
    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );
    let runs = system
        .process(pid)
        .is_some_and(|p| p.cmd().iter().any(|a| a == arg));
    if runs {
        Snapshot::now().kill_trees([pid.as_u32()]);
    }
}

pub fn own_exe() -> Result<std::path::PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(exe
        .to_str()
        .and_then(|path| path.strip_suffix(" (deleted)"))
        .map_or(exe.clone(), std::path::PathBuf::from))
}

const ANNOUNCE_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

pub fn announced(mut cmd: std::process::Command, input: &[u8]) -> Result<String, String> {
    use std::io::{BufRead, Write};
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    crate::timed::own_group(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let sent = child
        .stdin
        .take()
        .expect("stdin을 piped로 열었다")
        .write_all(input);
    let stdout = child.stdout.take().expect("stdout을 piped로 열었다");
    let (read, line) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let got = std::io::BufReader::new(stdout).read_line(&mut line);
        let _ = read.send(got.map(|_| line));
    });
    let announced = line.recv_timeout(ANNOUNCE_WAIT);
    if announced.is_err() {
        let _ = child.kill();
    }
    std::thread::spawn(move || child.wait());
    let line = announced.map_err(|_| {
        format!(
            "did not announce its address within {} seconds",
            ANNOUNCE_WAIT.as_secs()
        )
    })?;
    sent.and(line)
        .map(|line| line.trim().to_owned())
        .map_err(|e| e.to_string())
}

pub fn free_port() -> Result<u16, String> {
    std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| e.to_string())
}

pub enum Listen {
    Port(u16),
    #[cfg(unix)]
    Socket(std::path::PathBuf),
}

impl Listen {
    async fn open(&self) -> bool {
        match self {
            Listen::Port(port) => {
                tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, *port))
                    .await
                    .is_ok()
            }
            #[cfg(unix)]
            Listen::Socket(path) => tokio::net::UnixStream::connect(path).await.is_ok(),
        }
    }
}

pub async fn until_listening(exited: impl Future<Output = ()>, at: &Listen) -> Result<(), String> {
    const POLL: std::time::Duration = std::time::Duration::from_millis(100);
    tokio::pin!(exited);
    loop {
        if at.open().await {
            return Ok(());
        }
        tokio::select! {
            () = &mut exited => return Err("exited before listening".into()),
            _ = tokio::time::sleep(POLL) => {}
        }
    }
}

fn child_on_path(parents: &HashMap<u32, u32>, root: u32, pid: u32) -> Option<u32> {
    let mut at = pid;
    for _ in 0..parents.len() {
        match parents.get(&at) {
            Some(&parent) if parent == root => return Some(at),
            Some(&parent) => at = parent,
            None => return None,
        }
    }
    None
}

fn subtree(parents: &HashMap<u32, u32>, root: u32) -> Vec<u32> {
    let mut found = vec![root];
    let mut next = 0;
    while next < found.len() {
        let at = found[next];
        let children: Vec<u32> = parents
            .iter()
            .filter(|&(child, &parent)| parent == at && !found.contains(child))
            .map(|(&child, _)| child)
            .collect();
        found.extend(children);
        next += 1;
    }
    found
}
