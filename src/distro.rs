use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::task::JoinSet;

use crate::protocol::DISTRO_HOST;
use crate::{process_tree, tunnel};

const MAGIC: &[u8] = b"loom-distro";
const READY: &str = "loom distro-host ready";
const MAX_FRAME: usize = 16 << 20;
const MAX_TEXT: usize = 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const GREET_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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
    folder: String,
    program: String,
    gates: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    up: tokio::sync::Notify,
}

#[derive(Clone)]
pub struct Distro {
    distros: Arc<Distros>,
    name: String,
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

    pub fn through_wsl(dir: PathBuf, loom: &Path) -> Self {
        let folder = loom
            .parent()
            .unwrap_or(Path::new("."))
            .display()
            .to_string();
        let program = format!(
            "./{}",
            loom.file_name().unwrap_or_default().to_string_lossy()
        );
        Self {
            dir,
            folder,
            program,
            gates: Mutex::default(),
            up: tokio::sync::Notify::new(),
        }
    }

    pub fn distro(self: &Arc<Self>, name: &str) -> Distro {
        Distro {
            distros: self.clone(),
            name: name.to_owned(),
        }
    }

    pub fn reach(&self, distro: &str, at: SocketAddr) -> Result<TcpStream, Unreached> {
        let mut link = self
            .ask(distro, &Ask::Reach(at))
            .map_err(Unreached::Failed)?;
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

    pub fn start(&self, distro: &str, start: Start) -> Result<SocketAddr, String> {
        match self.answer(distro, &Ask::Start(start))? {
            Answer::Started(at) => Ok(at),
            other => Err(unexpected(other)),
        }
    }

    pub fn run(&self, distro: &str, command: Command) -> Result<Ran, String> {
        match self.answer(distro, &Ask::Run(command))? {
            Answer::Ran(ran) => Ok(ran),
            other => Err(unexpected(other)),
        }
    }

    pub fn serve(&self, distro: &str, command: Command) -> Result<Serving, String> {
        let mut link = self.ask(distro, &Ask::Serve(command))?;
        match read_answer(&mut link).map_err(|e| format!("distro host: {e}"))? {
            Answer::Spawned(pid) => Ok(Serving { pid, link }),
            other => Err(unexpected(other)),
        }
    }

    pub fn end(&self, distro: &str, roots: Vec<u32>) -> Result<(), String> {
        match self.answer(distro, &Ask::End(roots))? {
            Answer::Ended => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    fn answer(&self, distro: &str, ask: &Ask) -> Result<Answer, String> {
        let mut link = self.ask(distro, ask)?;
        read_answer(&mut link).map_err(|e| format!("distro host: {e}"))
    }

    fn ask(&self, distro: &str, ask: &Ask) -> Result<TcpStream, String> {
        let mut link = self.link(distro)?;
        let body = serde_json::to_vec(ask).map_err(|e| e.to_string())?;
        write_frame(&mut link, &body).map_err(|e| format!("distro host: {e}"))?;
        Ok(link)
    }

    fn link(&self, distro: &str) -> Result<TcpStream, String> {
        let gate = self
            .gates
            .lock()
            .unwrap()
            .entry(distro.to_owned())
            .or_default()
            .clone();
        let _one_start = gate.lock().unwrap();
        let file = self.dir.join(distro);
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
        let argv = [
            "wsl.exe",
            "-d",
            distro,
            "--cd",
            &self.folder,
            "--exec",
            &self.program,
            "distro-host",
            "--inner",
        ];
        let hosted = launch_outer(argv.map(str::to_owned).to_vec())
            .map_err(|cause| format!("{distro}: {cause}"))?;
        std::fs::create_dir_all(&self.dir)
            .and_then(|()| std::fs::write(&file, hosted.text()))
            .map_err(|e| format!("{}: {e}", file.display()))?;
        let link = greet(hosted.at).map_err(|e| format!("{distro}: distro host: {e}"))?;
        self.up.notify_one();
        Ok(link)
    }
}

impl Hosted {
    fn read(text: &str) -> Option<Hosted> {
        let mut words = text.split_whitespace();
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
        &self.name
    }

    pub fn run(&self, command: Command) -> Result<Ran, String> {
        self.distros.run(&self.name, command)
    }

    pub fn serve(&self, command: Command) -> Result<Serving, String> {
        self.distros.serve(&self.name, command)
    }

    pub fn end(&self, roots: Vec<u32>) -> Result<(), String> {
        self.distros.end(&self.name, roots)
    }
}

impl std::fmt::Debug for Distro {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

impl PartialEq for Distro {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
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
    let body = serde_json::to_vec(answer).map_err(|e| invalid(&e.to_string()))?;
    to.write_all(&frame(&body)).await
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
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
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
        Ask::Run(command) => match process(&command).map(|mut cmd| cmd.output()) {
            Ok(output) => match output.await {
                Ok(out) => Answer::Ran(Ran {
                    code: out.status.code(),
                    stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                }),
                Err(e) => Answer::Failed(format!("{}: {e}", command.argv[0])),
            },
            Err(e) => Answer::Failed(e),
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
    #[cfg(unix)]
    cmd.process_group(0);
    cmd.spawn().map_err(|e| format!("{}: {e}", command.argv[0]))
}
