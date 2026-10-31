use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::task::JoinSet;

use crate::protocol::DISTRO_HOST;
use crate::ssh::{self, Failure, MacRelease, Remote};
use crate::{process_tree, tunnel};

pub const LINUX_LOOM: &str = "loom-linux";
const LINUX_HASH_HEX: usize = 12;
const LINUX_MISSING: &str = "loom-linux is not next to loom.exe";
const SSH_FILE: &str = "ssh-";
const MAGIC: &[u8] = b"loom-distro";
const READY: &str = "loom distro-host ready";
const MAX_FRAME: usize = 16 << 20;
pub const MAX_FILE: usize = MAX_FRAME / 2;
const MAX_TEXT: usize = 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const GREET_TIMEOUT: Duration = Duration::from_secs(2);
const FILE_WAIT: Duration = Duration::from_secs(30);
pub const SSH_NOT_CONNECTED: &str = "SSH hosts are not connected yet";

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Host {
    Distro(String),
    Ssh(String),
}

impl Host {
    pub fn name(&self) -> &str {
        match self {
            Host::Distro(name) | Host::Ssh(name) => name,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
}

#[derive(Debug, PartialEq)]
struct Bytes(Vec<u8>);

impl Serialize for Bytes {
    fn serialize<S: Serializer>(&self, to: S) -> Result<S::Ok, S::Error> {
        to.serialize_str(&base64::engine::general_purpose::STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: Deserializer<'de>>(from: D) -> Result<Self, D::Error> {
        let text = String::deserialize(from)?;
        base64::engine::general_purpose::STANDARD
            .decode(text)
            .map(Bytes)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Written {
    path: String,
    bytes: Bytes,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Command {
    pub argv: Vec<String>,
    pub cwd: String,
    pub env: Vec<(String, String)>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Start {
    pub program: String,
    pub command: Command,
    pub setup: Option<Vec<String>>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Ask {
    Start(Start),
    Reach(SocketAddr),
    Run(Command),
    Serve(Command),
    End(Vec<u32>),
    Read(String),
    Write(Written),
    List(String),
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Answer {
    Started(SocketAddr),
    Reached,
    Gone,
    Ran(Ran),
    Spawned(u32),
    Exited(Option<i32>),
    Ended,
    Read(Bytes),
    Written,
    Listed(Vec<Entry>),
    Missing,
    Failed(String),
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Ran {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, PartialEq)]
pub enum Unreached {
    Gone,
    Failed(String),
}

struct Hosted {
    at: SocketAddr,
    pid: Option<u32>,
}

pub struct Distros {
    dir: PathBuf,
    linux: Option<PathBuf>,
    mac: MacRelease,
    gates: Mutex<HashMap<Host, Arc<Mutex<()>>>>,
    remotes: Mutex<HashMap<Host, Remote>>,
    up: tokio::sync::Notify,
}

pub struct Held(TcpStream);

#[derive(Clone)]
pub struct Distro {
    distros: Arc<Distros>,
    host: Host,
}

pub struct Serving {
    pub pid: u32,
    link: TcpStream,
}

#[derive(Deserialize, Serialize)]
struct Launcher {
    argv: Vec<String>,
}

impl Distros {
    pub fn up(&self) -> tokio::sync::futures::Notified<'_> {
        self.up.notified()
    }

    pub fn new(dir: PathBuf, linux: Option<PathBuf>, mac: MacRelease) -> Self {
        Self {
            dir,
            linux,
            mac,
            gates: Mutex::default(),
            remotes: Mutex::default(),
            up: tokio::sync::Notify::new(),
        }
    }

    pub fn host(self: &Arc<Self>, host: Host) -> Distro {
        Distro {
            distros: self.clone(),
            host,
        }
    }

    pub fn reach(&self, host: &Host, at: SocketAddr) -> Result<TcpStream, Unreached> {
        let mut link = self.ask(host, &Ask::Reach(at)).map_err(Unreached::Failed)?;
        let answer = link
            .set_read_timeout(Some(CONNECT_TIMEOUT))
            .and_then(|()| read_answer(&mut link))
            .and_then(|answer| link.set_read_timeout(None).map(|()| answer));
        match answer {
            Ok(Answer::Reached) => Ok(link),
            Ok(Answer::Gone) => Err(Unreached::Gone),
            Ok(other) => Err(Unreached::Failed(unexpected(other))),
            Err(e) => Err(Unreached::Failed(format!("distro host: {e}"))),
        }
    }

    pub fn start(&self, host: &Host, start: Start) -> Result<SocketAddr, String> {
        match self.answer(host, &Ask::Start(start))? {
            Answer::Started(at) => Ok(at),
            other => Err(unexpected(other)),
        }
    }

    pub fn run_within(
        &self,
        host: &Host,
        command: Command,
        limit: Duration,
    ) -> Result<Ran, String> {
        let program = command.argv.first().cloned().unwrap_or_default();
        let mut link = self.ask(host, &Ask::Run(command))?;
        let answer = link
            .set_read_timeout(Some(limit))
            .and_then(|()| read_answer(&mut link));
        match answer {
            Ok(Answer::Ran(ran)) => Ok(ran),
            Ok(other) => Err(unexpected(other)),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                Err(format!(
                    "{program} on {} did not end in {} s, so loom ended it",
                    host.name(),
                    limit.as_secs()
                ))
            }
            Err(e) => Err(format!("distro host: {e}")),
        }
    }

    pub fn serve(&self, host: &Host, command: Command) -> Result<Serving, String> {
        let mut link = self.ask(host, &Ask::Serve(command))?;
        match read_answer(&mut link).map_err(|e| format!("distro host: {e}"))? {
            Answer::Spawned(pid) => Ok(Serving { pid, link }),
            other => Err(unexpected(other)),
        }
    }

    pub fn end(&self, host: &Host, roots: Vec<u32>) -> Result<(), String> {
        match self.answer(host, &Ask::End(roots))? {
            Answer::Ended => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    pub fn read(&self, host: &Host, path: &str) -> io::Result<Vec<u8>> {
        match self.file_answer(host, &Ask::Read(path.to_owned()))? {
            Answer::Read(Bytes(bytes)) => Ok(bytes),
            Answer::Missing => Err(io::ErrorKind::NotFound.into()),
            other => Err(io::Error::other(unexpected(other))),
        }
    }

    pub fn write(&self, host: &Host, path: &str, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() > MAX_FILE {
            return Err(too_large());
        }
        let ask = Ask::Write(Written {
            path: path.to_owned(),
            bytes: Bytes(bytes.to_vec()),
        });
        match self.file_answer(host, &ask)? {
            Answer::Written => Ok(()),
            other => Err(io::Error::other(unexpected(other))),
        }
    }

    pub fn list(&self, host: &Host, dir: &str) -> io::Result<Vec<Entry>> {
        match self.file_answer(host, &Ask::List(dir.to_owned()))? {
            Answer::Listed(entries) => Ok(entries),
            Answer::Missing => Err(io::ErrorKind::NotFound.into()),
            other => Err(io::Error::other(unexpected(other))),
        }
    }

    pub fn hold(&self, host: &Host) -> Result<Held, Failure> {
        self.link(host).map(Held)
    }

    pub fn release(&self, host: &Host) {
        let gate = self.gate(host);
        let _one_start = gate.lock().unwrap();
        self.remotes.lock().unwrap().remove(host);
        let file = self.dir.join(file_name(host));
        let known = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| Hosted::read(&text));
        if let Some(pid) = known.and_then(|known| known.pid) {
            process_tree::end_if_runs(pid, "distro-host");
        }
        let _ = std::fs::remove_file(&file);
    }

    pub fn remote(&self, host: &Host) -> Result<Remote, Failure> {
        let Host::Ssh(name) = host else {
            return Err(Failure::Failed(format!("{}: not an SSH host", host.name())));
        };
        self.link(host)?;
        let gate = self.gate(host);
        let _one_start = gate.lock().unwrap();
        if let Some(remote) = self.remotes.lock().unwrap().get(host) {
            return Ok(remote.clone());
        }
        let outer = self.prepare(name)?;
        self.remotes
            .lock()
            .unwrap()
            .insert(host.clone(), outer.remote.clone());
        Ok(outer.remote)
    }

    fn prepare(&self, name: &str) -> Result<ssh::Outer, Failure> {
        let linux = self.linux.as_deref();
        let file = linux
            .map(linux_name)
            .transpose()
            .map_err(|e| Failure::Failed(format!("{name}: {e}")))?;
        ssh::prepare(name, linux.zip(file.as_deref()), self.mac)
    }

    fn gate(&self, host: &Host) -> Arc<Mutex<()>> {
        self.gates
            .lock()
            .unwrap()
            .entry(host.clone())
            .or_default()
            .clone()
    }

    fn file_answer(&self, host: &Host, ask: &Ask) -> io::Result<Answer> {
        let mut link = self
            .link(host)
            .map_err(|e| io::Error::other(e.to_string()))?;
        link.set_read_timeout(Some(FILE_WAIT))?;
        link.set_write_timeout(Some(FILE_WAIT))?;
        let body = serde_json::to_vec(ask).map_err(io::Error::other)?;
        write_frame(&mut link, &body)?;
        read_answer(&mut link)
    }

    fn answer(&self, host: &Host, ask: &Ask) -> Result<Answer, String> {
        let mut link = self.ask(host, ask)?;
        read_answer(&mut link).map_err(|e| format!("distro host: {e}"))
    }

    fn ask(&self, host: &Host, ask: &Ask) -> Result<TcpStream, String> {
        let mut link = self.link(host).map_err(|e| e.to_string())?;
        let body = serde_json::to_vec(ask).map_err(|e| e.to_string())?;
        write_frame(&mut link, &body).map_err(|e| format!("distro host: {e}"))?;
        Ok(link)
    }

    fn link(&self, host: &Host) -> Result<TcpStream, Failure> {
        let gate = self.gate(host);
        let _one_start = gate.lock().unwrap();
        let file = self.dir.join(file_name(host));
        let known = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| Hosted::read(&text));
        if let Some(known) = known {
            if let Ok(link) = greet(known.at) {
                return Ok(link);
            }
            if let Some(pid) = known.pid {
                process_tree::end_if_runs(pid, "distro-host");
            }
        }
        let failed = |cause| Failure::Failed(format!("{}: {cause}", host.name()));
        let (argv, released) = match host {
            Host::Distro(distro) => (self.through_wsl(distro).map_err(failed)?, false),
            Host::Ssh(name) => {
                let outer = self.prepare(name)?;
                self.remotes
                    .lock()
                    .unwrap()
                    .insert(host.clone(), outer.remote);
                (outer.argv, outer.released)
            }
        };
        let hosted = launch_outer(argv).map_err(failed)?;
        std::fs::create_dir_all(&self.dir)
            .and_then(|()| std::fs::write(&file, hosted.text()))
            .map_err(|e| Failure::Failed(format!("{}: {e}", file.display())))?;
        let link = greet(hosted.at).map_err(|e| match released {
            true => Failure::VersionMismatch,
            false => failed(format!("distro host: {e}")),
        })?;
        self.up.notify_one();
        Ok(link)
    }

    fn through_wsl(&self, distro: &str) -> Result<Vec<String>, String> {
        let loom = self.linux.as_deref().ok_or(LINUX_MISSING)?;
        let folder = loom
            .parent()
            .unwrap_or(Path::new("."))
            .display()
            .to_string();
        let program = format!(
            "./{}",
            loom.file_name().unwrap_or_default().to_string_lossy()
        );
        let argv = [
            "wsl.exe",
            "-d",
            distro,
            "--cd",
            &folder,
            "--exec",
            &program,
            "distro-host",
            "--inner",
        ];
        Ok(argv.map(str::to_owned).to_vec())
    }
}

fn file_name(host: &Host) -> String {
    match host {
        Host::Distro(name) => name.clone(),
        Host::Ssh(name) => format!("{SSH_FILE}{name}"),
    }
}

pub fn linux_name(file: &Path) -> io::Result<String> {
    use sha2::Digest;
    let mut hash = sha2::Sha256::new();
    let mut from = std::fs::File::open(file)?;
    let mut buf = vec![0u8; 1 << 16];
    loop {
        match from.read(&mut buf)? {
            0 => break,
            n => hash.update(&buf[..n]),
        }
    }
    let hex: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("{LINUX_LOOM}-{}", &hex[..LINUX_HASH_HEX]))
}

impl Held {
    pub fn closed(mut self) {
        let mut byte = [0u8; 1];
        while let Ok(1..) = self.0.read(&mut byte) {}
    }
}

impl Hosted {
    fn read(text: &str) -> Option<Hosted> {
        let mut words = text.lines().next()?.split_whitespace();
        Some(Hosted {
            at: words.next()?.parse().ok()?,
            pid: words.next().and_then(|pid| pid.parse().ok()),
        })
    }

    fn text(&self) -> String {
        match self.pid {
            Some(pid) => format!("{} {pid}", self.at),
            None => self.at.to_string(),
        }
    }
}

impl Distro {
    pub fn name(&self) -> &str {
        self.host.name()
    }

    pub fn host(&self) -> &Host {
        &self.host
    }

    pub fn hosts(&self) -> &Arc<Distros> {
        &self.distros
    }

    pub fn run_within(&self, command: Command, limit: Duration) -> Result<Ran, String> {
        self.distros.run_within(&self.host, command, limit)
    }

    pub fn serve(&self, command: Command) -> Result<Serving, String> {
        self.distros.serve(&self.host, command)
    }

    pub fn end(&self, roots: Vec<u32>) -> Result<(), String> {
        self.distros.end(&self.host, roots)
    }
}

impl std::fmt::Debug for Distro {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl PartialEq for Distro {
    fn eq(&self, other: &Self) -> bool {
        self.host == other.host
    }
}

impl Serving {
    pub fn exited(&self) -> Result<Option<i32>, String> {
        match read_answer(&mut &self.link).map_err(|e| format!("distro host: {e}"))? {
            Answer::Exited(code) => Ok(code),
            other => Err(unexpected(other)),
        }
    }

    pub fn hang_up(&self) {
        let _ = self.link.shutdown(std::net::Shutdown::Both);
    }
}

fn unexpected(answer: Answer) -> String {
    match answer {
        Answer::Failed(cause) => cause,
        other => format!("distro host answered {other:?}"),
    }
}

fn launch_outer(argv: Vec<String>) -> Result<Hosted, String> {
    let mut outer = std::process::Command::new(process_tree::own_exe()?);
    outer.arg("distro-host");
    let spec = serde_json::to_vec(&Launcher { argv }).map_err(|e| e.to_string())?;
    let line = process_tree::announced(outer, &spec).map_err(|e| format!("distro host: {e}"))?;
    Hosted::read(&line).ok_or(match line.is_empty() {
        true => "distro host: exited before announcing its address".to_owned(),
        false => line,
    })
}

fn greet(at: SocketAddr) -> io::Result<TcpStream> {
    let mut link = TcpStream::connect_timeout(&at, CONNECT_TIMEOUT)?;
    link.set_read_timeout(Some(GREET_TIMEOUT))?;
    write_frame(&mut link, &hello(DISTRO_HOST))?;
    match read_frame(&mut link)? {
        body if body == hello(DISTRO_HOST) => {}
        _ => return Err(invalid("not a distro host of this version")),
    }
    link.set_read_timeout(None)?;
    Ok(link)
}

fn hello(version: u32) -> Vec<u8> {
    [MAGIC, &version.to_le_bytes()].concat()
}

fn version_of(hello: &[u8]) -> Option<u32> {
    let version = hello.strip_prefix(MAGIC)?;
    Some(u32::from_le_bytes(version.try_into().ok()?))
}

fn frame(body: &[u8]) -> Vec<u8> {
    [&(body.len() as u32).to_le_bytes()[..], body].concat()
}

fn write_frame(to: &mut impl Write, body: &[u8]) -> io::Result<()> {
    to.write_all(&frame(body))
}

fn read_frame(from: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    from.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(invalid("frame too long"));
    }
    let mut body = vec![0u8; len];
    from.read_exact(&mut body)?;
    Ok(body)
}

fn read_answer(from: &mut impl Read) -> io::Result<Answer> {
    serde_json::from_slice(&read_frame(from)?).map_err(|e| invalid(&e.to_string()))
}

async fn read_frame_async(from: &mut (impl AsyncRead + Unpin)) -> io::Result<Vec<u8>> {
    let len = from.read_u32_le().await? as usize;
    if len > MAX_FRAME {
        return Err(invalid("frame too long"));
    }
    let mut body = vec![0u8; len];
    from.read_exact(&mut body).await?;
    Ok(body)
}

async fn write_answer(to: &mut (impl AsyncWrite + Unpin), answer: &Answer) -> io::Result<()> {
    let mut body = serde_json::to_vec(answer).map_err(|e| invalid(&e.to_string()))?;
    if body.len() > MAX_FRAME {
        body = serde_json::to_vec(&Answer::Failed(too_large().to_string()))
            .map_err(|e| invalid(&e.to_string()))?;
    }
    to.write_all(&frame(&body)).await
}

fn too_large() -> io::Error {
    io::Error::other(format!("larger than the {MAX_FILE}-byte limit"))
}

pub fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let real = std::fs::canonicalize(path);
    let path = real.as_deref().unwrap_or(path);
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(io::Error::other(format!(
            "{}: not a file path",
            path.display()
        )));
    };
    std::fs::create_dir_all(parent)?;
    let draft = path.with_file_name(format!(
        "{}.{}.{}.loom",
        name.to_string_lossy(),
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let written = std::fs::write(&draft, bytes)
        .and_then(|()| match std::fs::metadata(path) {
            Ok(old) => std::fs::set_permissions(&draft, old.permissions()),
            Err(_) => Ok(()),
        })
        .and_then(|()| std::fs::rename(&draft, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&draft);
    }
    written
}

pub fn list(dir: &Path) -> io::Result<Vec<Entry>> {
    Ok(std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|entry| Entry {
            name: entry.file_name().to_string_lossy().into_owned(),
            dir: entry.path().is_dir(),
        })
        .collect())
}

fn read_bounded(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_FILE as u64 + 1)
        .read_to_end(&mut bytes)?;
    match bytes.len() > MAX_FILE {
        true => Err(too_large()),
        false => Ok(bytes),
    }
}

async fn on_file<T: Send + 'static>(
    path: &str,
    work: impl FnOnce(&Path) -> io::Result<T> + Send + 'static,
    answer: impl FnOnce(T) -> Answer,
) -> Answer {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Answer::Failed(format!("{}: not an absolute path", path.display()));
    }
    let shown = path.display().to_string();
    match tokio::task::spawn_blocking(move || work(&path)).await {
        Ok(Ok(done)) => answer(done),
        Ok(Err(e)) if e.kind() == io::ErrorKind::NotFound => Answer::Missing,
        Ok(Err(e)) => Answer::Failed(format!("{shown}: {e}")),
        Err(e) => Answer::Failed(e.to_string()),
    }
}

fn invalid(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.to_owned())
}

pub fn outer() -> Result<(), String> {
    let launcher: Launcher =
        serde_json::from_reader(std::io::stdin()).map_err(|e| format!("launcher: {e}"))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async move {
        let (program, args) = launcher.argv.split_first().ok_or("empty launcher")?;
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args)
            .env("WSL_UTF8", "1")
            .env("WSLENV", "")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(false);
        crate::timed::own_session(cmd.as_std_mut());
        let mut child = cmd.spawn().map_err(|e| format!("{program}: {e}"))?;
        let to_inner = child.stdin.take().expect("stdin을 piped로 열었다");
        let mut from_inner = child.stdout.take().expect("stdout을 piped로 열었다");
        let mut errors = child.stderr.take().expect("stderr를 piped로 열었다");
        let stderr = tokio::spawn(async move {
            let mut text = Vec::new();
            let mut buf = [0u8; 4096];
            while let Ok(n @ 1..) = errors.read(&mut buf).await {
                if text.len() < MAX_TEXT {
                    text.extend_from_slice(&buf[..n]);
                }
            }
            text
        });
        let first = first_line(&mut from_inner).await;
        if first.as_deref() != Some(READY.as_bytes()) {
            let mut out = first.unwrap_or_default();
            let _ = (&mut from_inner)
                .take(MAX_TEXT as u64)
                .read_to_end(&mut out)
                .await;
            let status = child.wait().await.map_err(|e| e.to_string())?;
            let err = stderr.await.unwrap_or_default();
            let text = [out, err]
                .iter()
                .map(|bytes| String::from_utf8_lossy(bytes).trim().to_owned())
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let line = format!("{text} ({status})");
            announce(line.split_whitespace().collect::<Vec<_>>().join(" "))?;
            return Ok(());
        }
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|e| e.to_string())?;
        let at = listener.local_addr().map_err(|e| e.to_string())?;
        announce(format!("{at} {}", std::process::id()))?;
        carry(listener, from_inner, to_inner).await;
        let _ = child.wait().await;
        Ok(())
    })
}

async fn first_line(from: &mut (impl AsyncRead + Unpin)) -> Option<Vec<u8>> {
    let mut line = Vec::new();
    loop {
        match from.read_u8().await {
            Ok(b'\n') => return Some(line),
            Ok(byte) if line.len() < MAX_TEXT => line.push(byte),
            Ok(_) => return Some(line),
            Err(_) if line.is_empty() => return None,
            Err(_) => return Some(line),
        }
    }
}

fn announce(line: impl std::fmt::Display) -> Result<(), String> {
    let mut stdout = std::io::stdout();
    writeln!(stdout, "{line}")
        .and_then(|()| stdout.flush())
        .map_err(|e| e.to_string())
}

pub(crate) async fn carry<R, W>(listener: tokio::net::TcpListener, from_inner: R, to_inner: W)
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (mux, mut events) = tunnel::start(from_inner, to_inner);
    let mut carried = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                if let Ok((conn, _)) = accepted {
                    carried.spawn(tunnel::splice(conn, mux.open()));
                }
            }
            Some(_) = carried.join_next() => {}
            event = events.recv() => if event.is_none() {
                break;
            },
        }
    }
    drop(listener);
    carried.join_all().await;
}

pub type Starter = fn(Start) -> Result<SocketAddr, String>;

pub fn inner(start: Starter) -> Result<(), String> {
    announce(READY)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(serve(tokio::io::stdin(), tokio::io::stdout(), start));
    runtime.shutdown_background();
    Ok(())
}

pub(crate) async fn serve<R, W>(from_outer: R, to_outer: W, start: Starter)
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let mut streams = tunnel::accept(from_outer, to_outer);
    let mut answering = JoinSet::new();
    loop {
        tokio::select! {
            stream = streams.recv() => match stream {
                Some(stream) => drop(answering.spawn(answer(stream, start))),
                None => break,
            },
            Some(_) = answering.join_next() => {}
        }
    }
}

async fn answer(mut stream: tunnel::Stream, start: Starter) {
    let Some(version) = read_frame_async(&mut stream)
        .await
        .ok()
        .and_then(|hello| version_of(&hello))
    else {
        return;
    };
    if version != DISTRO_HOST {
        std::process::exit(0);
    }
    let asked = match stream.write_all(&frame(&hello(DISTRO_HOST))).await {
        Ok(()) => read_frame_async(&mut stream).await,
        Err(e) => Err(e),
    };
    let Some(ask) = asked
        .ok()
        .and_then(|body| serde_json::from_slice::<Ask>(&body).ok())
    else {
        return;
    };
    let answer = match ask {
        Ask::Reach(at) => match tokio::net::TcpStream::connect(at).await {
            Ok(conn) => {
                if write_answer(&mut stream, &Answer::Reached).await.is_ok() {
                    tunnel::splice(stream, conn).await;
                }
                return;
            }
            Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => Answer::Gone,
            Err(e) => Answer::Failed(format!("{at}: {e}")),
        },
        Ask::Start(spec) => match tokio::task::spawn_blocking(move || start(spec)).await {
            Ok(Ok(at)) => Answer::Started(at),
            Ok(Err(e)) => Answer::Failed(e),
            Err(e) => Answer::Failed(e.to_string()),
        },
        Ask::Run(command) => match run_until_hung_up(&mut stream, &command).await {
            Some(answer) => answer,
            None => return,
        },
        Ask::End(roots) => match tokio::task::spawn_blocking(move || {
            process_tree::Snapshot::now().end_trees(roots)
        })
        .await
        {
            Ok(Ok(())) => Answer::Ended,
            Ok(Err(e)) => Answer::Failed(e),
            Err(e) => Answer::Failed(e.to_string()),
        },
        Ask::Read(path) => on_file(&path, read_bounded, |bytes| Answer::Read(Bytes(bytes))).await,
        Ask::Write(Written { path, bytes }) => {
            on_file(
                &path,
                move |path| replace(path, &bytes.0),
                |()| Answer::Written,
            )
            .await
        }
        Ask::List(dir) => on_file(&dir, list, Answer::Listed).await,
        Ask::Serve(command) => {
            match process(&command).and_then(|cmd| spawn_detached(cmd, &command)) {
                Ok(mut child) => {
                    let pid = child.id().unwrap_or_default();
                    let _ = write_answer(&mut stream, &Answer::Spawned(pid)).await;
                    Answer::Exited(child.wait().await.ok().and_then(|s| s.code()))
                }
                Err(e) => Answer::Failed(e),
            }
        }
    };
    let _ = write_answer(&mut stream, &answer).await;
}

struct Running(Option<u32>);

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            process_tree::Snapshot::now().kill_trees([pid]);
        }
    }
}

async fn run_until_hung_up(stream: &mut tunnel::Stream, command: &Command) -> Option<Answer> {
    let spawned = process(command).and_then(|mut cmd| {
        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("{}: {e}", command.argv[0]))
    });
    let child = match spawned {
        Ok(child) => child,
        Err(e) => return Some(Answer::Failed(e)),
    };
    let mut running = Running(child.id());
    let mut byte = [0u8; 1];
    tokio::select! {
        output = child.wait_with_output() => {
            running.0 = None;
            Some(match output {
                Ok(out) => Answer::Ran(Ran {
                    code: out.status.code(),
                    stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                }),
                Err(e) => Answer::Failed(format!("{}: {e}", command.argv[0])),
            })
        }
        _ = stream.read(&mut byte) => None,
    }
}

fn process(command: &Command) -> Result<tokio::process::Command, String> {
    let (program, args) = command.argv.split_first().ok_or("empty command")?;
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .envs(command.env.iter().map(|(k, v)| (k, v)))
        .current_dir(&command.cwd)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(false);
    Ok(cmd)
}

fn spawn_detached(
    mut cmd: tokio::process::Command,
    command: &Command,
) -> Result<tokio::process::Child, String> {
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    crate::timed::own_group(cmd.as_std_mut());
    cmd.spawn().map_err(|e| format!("{}: {e}", command.argv[0]))
}
