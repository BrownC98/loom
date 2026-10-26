mod endpoint;
mod host;
mod wire;

use std::collections::{HashMap, VecDeque};
use std::fmt::Display;
use std::hash::Hash;
use std::net::{SocketAddr, TcpStream};
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use bytes::Bytes;
use portable_pty::CommandBuilder;
use tokio::sync::{broadcast, watch};

use crate::distro::{self, Distros, Unreached};
use crate::process_tree::{self, Snapshot};
use crate::screen::Screen;
use wire::Frame;

pub use host::run;

pub type Links<K> = tokio::sync::mpsc::UnboundedSender<(K, Linked)>;

pub enum Linked {
    Terminal,
    Streams { link: TcpStream, held: Option<u32> },
    EndpointFailed(String),
}

const BACKLOG_BYTES: usize = 256 * 1024;
const VIEWER_QUEUE_CHUNKS: usize = 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const GREET_TIMEOUT: Duration = Duration::from_secs(2);
const LOST_CODE: u32 = u32::MAX;

const SETTLED_AFTER: Duration = Duration::from_secs(2);

const TAIL_LINES: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Exit {
    Command(u32),
    Setup(u32),
    Lost,
}

impl Exit {
    fn frame(self) -> Frame {
        match self {
            Exit::Command(code) => Frame::Exit(code),
            Exit::Setup(code) => Frame::SetupFailed(code),
            Exit::Lost => Frame::Exit(LOST_CODE),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Unattached {
    Gone,
    Silent(String),
}

impl Display for Unattached {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unattached::Gone => f.write_str("pty-host is gone"),
            Unattached::Silent(cause) => f.write_str(cause),
        }
    }
}

pub struct Attached {
    pub backlog: Bytes,
    pub output: broadcast::Receiver<Bytes>,
    pub size: watch::Receiver<PtySize>,
    pub settled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
}

pub struct Kept {
    pub size: PtySize,
    pub backlog: Bytes,
}

#[derive(Clone)]
pub struct NodeHost<K> {
    dir: PathBuf,
    kept: PathBuf,
    nodes: Arc<RwLock<HashMap<K, Arc<Pty>>>>,
    links: Option<Links<K>>,
    distros: Option<Arc<Distros>>,
}

type OnExit = Box<dyn FnOnce(Exit) + Send>;

enum Address {
    Here(SocketAddr),
    Distro { at: SocketAddr, distro: String },
}

impl Address {
    fn read(file: &Path) -> Result<Address, Unattached> {
        let text = std::fs::read_to_string(file).map_err(|e| Unattached::Silent(e.to_string()))?;
        let mut lines = text.lines();
        let at = lines
            .next()
            .unwrap_or_default()
            .trim()
            .parse()
            .map_err(|e| Unattached::Silent(format!("{}: {e}", file.display())))?;
        Ok(match lines.next().map(str::trim) {
            Some(distro) if !distro.is_empty() => Address::Distro {
                at,
                distro: distro.to_owned(),
            },
            _ => Address::Here(at),
        })
    }
}

struct Pty {
    pid: Mutex<Option<u32>>,
    guarded: Mutex<Vec<u32>>,
    host: Mutex<TcpStream>,
    output: Mutex<Output>,
    size: watch::Sender<PtySize>,
    sizing: Mutex<Sizing>,
    #[cfg(test)]
    detached: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
struct Sizing {
    loan: Option<Loan>,
    host: HostSize,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum HostSize {
    #[default]
    Unreported,
    Claimed,
    Reported,
}

struct Loan {
    by: Borrower,
    before: PtySize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Borrower(u64);

impl Borrower {
    pub fn fresh() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

struct Output {
    backlog: Backlog,
    tx: broadcast::Sender<Bytes>,
    last_push: Instant,
}

#[derive(Default)]
struct Backlog {
    bytes: VecDeque<u8>,
    modes: Vec<(u16, bool)>,
}

const KEPT_MODES: [u16; 15] = [
    47, 1047, 1049, 1, 9, 1000, 1001, 1002, 1003, 1004, 1005, 1006, 1015, 1016, 2004,
];

impl<K: Copy + Eq + Hash + Display + Send + Sync + 'static> NodeHost<K> {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            kept: dir.with_extension("screens"),
            dir,
            nodes: Arc::default(),
            links: None,
            distros: None,
        }
    }

    pub fn with_distros(self, distros: Arc<Distros>) -> Self {
        Self {
            distros: Some(distros),
            ..self
        }
    }

    pub fn with_links(self, links: Links<K>) -> Self {
        Self {
            links: Some(links),
            ..self
        }
    }

    pub fn spawn(
        &self,
        id: K,
        cmd: CommandBuilder,
        setup: Option<CommandBuilder>,
        on_exit: impl FnOnce(Exit) + Send + 'static,
    ) -> Result<(), String> {
        self.start(id, host::Spec::of(&cmd, setup.as_ref()), on_exit)
    }

    pub fn spawn_in(
        &self,
        id: K,
        distro: &str,
        start: distro::Start,
        on_exit: impl FnOnce(Exit) + Send + 'static,
    ) -> Result<(), String> {
        let distros = self.distros.as_ref().ok_or("no WSL distro host")?;
        let at = distros.start(distro, start)?;
        self.keep_address(id, &format!("{at}\n{distro}"))?;
        self.resume(id, on_exit).map_err(|e| e.to_string())
    }

    fn start(
        &self,
        id: K,
        spec: host::Spec,
        on_exit: impl FnOnce(Exit) + Send + 'static,
    ) -> Result<(), String> {
        let address = start_host(&process_tree::own_exe()?, &spec)?;
        self.keep_address(id, &address)?;
        self.resume(id, on_exit).map_err(|e| e.to_string())
    }

    fn keep_address(&self, id: K, address: &str) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir)
            .and_then(|()| std::fs::write(self.file(id), address))
            .map_err(|e| e.to_string())
    }

    pub fn resume(
        &self,
        id: K,
        on_exit: impl FnOnce(Exit) + Send + 'static,
    ) -> Result<(), Unattached> {
        let _ = std::fs::remove_file(self.kept_file(id));
        self.link(id, Box::new(on_exit), None)
            .map_err(|(unattached, _)| unattached)
    }

    fn link(
        &self,
        id: K,
        on_exit: OnExit,
        replacing: Option<Arc<Pty>>,
    ) -> Result<(), (Unattached, OnExit)> {
        let file = self.file(id);
        let connected =
            Address::read(&file).and_then(|at| Ok((self.connect(&at, wire::greet)?, at)));
        let ((mut reader, greeted), at) = match connected {
            Ok(found) => found,
            Err(e) => {
                let _ = std::fs::remove_file(&file);
                return Err((e, on_exit));
            }
        };
        let pid = greeted.pid;
        let linked = match greeted.version >= wire::STREAMS_SINCE && self.links.is_some() {
            true => match self.connect(&at, |link| wire::greet_streams(link, greeted.version)) {
                Ok((link, held)) => Linked::Streams { link, held },
                Err(e) => {
                    let _ = wire::write(&mut reader, &Frame::Kill);
                    std::thread::spawn(move || while wire::read(&mut reader).is_ok() {});
                    let _ = std::fs::remove_file(&file);
                    return Err((
                        Unattached::Silent(format!("node endpoint link: {e}")),
                        on_exit,
                    ));
                }
            },
            false => Linked::Terminal,
        };
        let host = match reader.try_clone() {
            Ok(host) => host,
            Err(e) => return Err((Unattached::Silent(e.to_string()), on_exit)),
        };
        let pty = Arc::new(Pty {
            pid: Mutex::new(pid),
            guarded: Mutex::default(),
            host: Mutex::new(host),
            output: Mutex::new(Output {
                backlog: Backlog::default(),
                tx: broadcast::channel(VIEWER_QUEUE_CHUNKS).0,
                last_push: Instant::now(),
            }),
            size: watch::Sender::new(host::INITIAL_SIZE_VIEW),
            sizing: Mutex::default(),
            #[cfg(test)]
            detached: Default::default(),
        });
        let ours = {
            let mut nodes = self.nodes.write().unwrap();
            let ours = replacing
                .as_ref()
                .is_none_or(|old| nodes.get(&id).is_some_and(|now| Arc::ptr_eq(now, old)));
            if ours {
                nodes.insert(id, pty.clone());
            }
            ours
        };
        let links = self.links.clone().filter(|_| ours);
        if let Some(links) = &links {
            let _ = links.send((id, linked));
        }
        if !ours {
            pty.send(&Frame::Kill);
        }
        let host = self.clone();
        let carried_by_distro = matches!(at, Address::Distro { .. });
        std::thread::spawn(move || {
            let exit = loop {
                match wire::read(&mut reader) {
                    Ok(Frame::Data(chunk)) => {
                        pty.output.lock().unwrap().push(&chunk);
                    }
                    Ok(Frame::Exit(code)) => break Some(Exit::Command(code)),
                    Ok(Frame::SetupFailed(code)) => break Some(Exit::Setup(code)),
                    Ok(Frame::Pid(pid)) => *pty.pid.lock().unwrap() = Some(pid),
                    Ok(Frame::EndpointFailed(error)) => {
                        if let Some(links) = &links {
                            let _ = links.send((id, Linked::EndpointFailed(error)));
                        }
                    }
                    Ok(Frame::Resize { cols, rows }) => {
                        let mut sizing = pty.sizing.lock().unwrap();
                        if sizing.host == HostSize::Unreported {
                            pty.size.send_replace(PtySize { cols, rows });
                        }
                        sizing.host = HostSize::Reported;
                    }
                    Ok(_) => {}
                    Err(_) => break None,
                }
            };
            #[cfg(test)]
            if pty.detached.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            let on_exit = match exit {
                None if carried_by_distro => match host.link(id, on_exit, Some(pty)) {
                    Ok(()) => return,
                    Err((_, on_exit)) => on_exit,
                },
                _ => on_exit,
            };
            let _ = std::fs::remove_file(host.file(id));
            on_exit(exit.unwrap_or(Exit::Lost));
        });
        Ok(())
    }

    fn connect<G>(
        &self,
        at: &Address,
        greet: impl FnOnce(&mut TcpStream) -> std::io::Result<G>,
    ) -> Result<(TcpStream, G), Unattached> {
        let silent = |e: std::io::Error| Unattached::Silent(e.to_string());
        let mut stream = match at {
            Address::Here(at) => {
                TcpStream::connect_timeout(at, CONNECT_TIMEOUT).map_err(|e| match e.kind() {
                    std::io::ErrorKind::ConnectionRefused => Unattached::Gone,
                    _ => silent(e),
                })?
            }
            Address::Distro { at, distro } => self
                .distros
                .as_ref()
                .ok_or_else(|| Unattached::Silent(format!("no distro host for {distro}")))?
                .reach(distro, *at)
                .map_err(|e| match e {
                    Unreached::Gone => Unattached::Gone,
                    Unreached::Failed(cause) => Unattached::Silent(cause),
                })?,
        };
        stream
            .set_read_timeout(Some(GREET_TIMEOUT))
            .map_err(silent)?;
        let greeted = greet(&mut stream).map_err(silent)?;
        stream.set_read_timeout(None).map_err(silent)?;
        Ok((stream, greeted))
    }

    pub fn end_others(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let ours: Vec<String> = self
            .nodes
            .read()
            .unwrap()
            .keys()
            .map(|id| id.to_string())
            .collect();
        for entry in entries.flatten() {
            if ours.contains(&entry.file_name().to_string_lossy().into_owned()) {
                continue;
            }
            if let Ok((mut host, _)) =
                Address::read(&entry.path()).and_then(|at| self.connect(&at, wire::greet))
            {
                let _ = wire::write(&mut host, &Frame::Kill);
            }
            let _ = std::fs::remove_file(entry.path());
        }
    }

    #[cfg(all(test, unix))]
    pub fn detach(&self) {
        for pty in self.nodes.write().unwrap().drain().map(|(_, pty)| pty) {
            pty.detached
                .store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = pty.host.lock().unwrap().shutdown(std::net::Shutdown::Both);
        }
    }

    pub fn has_output(&self, id: K) -> bool {
        self.nodes
            .read()
            .unwrap()
            .get(&id)
            .is_none_or(|pty| !pty.output.lock().unwrap().backlog.bytes.is_empty())
    }

    pub fn tail(&self, id: K) -> String {
        let Some(pty) = self.pty(id) else {
            return String::new();
        };
        let bytes = pty.output.lock().unwrap().backlog.to_vec();
        let lines: Vec<String> = String::from_utf8_lossy(&bytes)
            .lines()
            .filter_map(|line| {
                line.split('\r')
                    .map(strip_ansi_escapes::strip_str)
                    .rfind(|part| !part.trim().is_empty())
            })
            .collect();
        lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n")
    }

    pub fn screen(&self, id: K) -> Screen {
        self.pty(id)
            .map_or_else(Screen::default, |pty| pty.screen())
    }

    pub fn attach(&self, id: K) -> Option<Attached> {
        let pty = self.pty(id)?;
        let output = pty.output.lock().unwrap();
        Some(Attached {
            backlog: Bytes::from(output.backlog.to_vec()),
            output: output.tx.subscribe(),
            size: pty.size.subscribe(),
            settled: output.settled(Instant::now()),
        })
    }

    pub fn write_if<E>(
        &self,
        id: K,
        pick: impl FnOnce(&Keys<'_>) -> Result<Vec<u8>, E>,
    ) -> Result<(), E> {
        let pty = self.pty(id);
        let mut host = pty.as_ref().map(|pty| pty.host.lock().unwrap());
        let bytes = pick(&Keys(pty.as_deref()))?;
        if let Some(host) = host.as_mut() {
            let _ = wire::write(&mut **host, &Frame::Data(bytes));
        }
        Ok(())
    }

    pub fn resize(&self, id: K, cols: u16, rows: u16) {
        if let Some(pty) = self.pty(id) {
            pty.change_size(|sizing, _| {
                sizing.loan = None;
                Some(PtySize { cols, rows })
            });
        }
    }

    pub fn borrow(&self, id: K, by: Borrower, cols: u16, rows: u16) {
        if let Some(pty) = self.pty(id) {
            pty.change_size(|sizing, now| {
                let before = sizing.loan.as_ref().map_or(now, |l| l.before);
                sizing.loan = Some(Loan { by, before });
                Some(PtySize { cols, rows })
            });
        }
    }

    pub fn give_back(&self, id: K, by: Borrower) {
        if let Some(pty) = self.pty(id) {
            pty.change_size(|sizing, _| {
                sizing.loan.take_if(|l| l.by == by).map(|loan| loan.before)
            });
        }
    }

    pub fn kill(&self, id: K) {
        if let Some(pty) = self.nodes.write().unwrap().remove(&id) {
            pty.send(&Frame::Kill);
        }
        let _ = std::fs::remove_file(self.kept_file(id));
    }

    pub fn keep(&self, id: K) -> std::io::Result<()> {
        let nodes = self.nodes.read().unwrap();
        let Some(pty) = nodes.get(&id) else {
            return Ok(());
        };
        let PtySize { cols, rows } = *pty.size.borrow();
        let backlog = pty.output.lock().unwrap().backlog.to_vec();
        let file = self.kept_file(id);
        let temp = file.with_extension("tmp");
        std::fs::create_dir_all(&self.kept)?;
        std::fs::write(
            &temp,
            [&cols.to_le_bytes()[..], &rows.to_le_bytes(), &backlog].concat(),
        )?;
        std::fs::rename(temp, file)
    }

    pub fn kept(&self, id: K) -> Option<Kept> {
        let saved = std::fs::read(self.kept_file(id)).ok()?;
        let (size, backlog) = saved.split_first_chunk::<4>()?;
        Some(Kept {
            size: PtySize {
                cols: u16::from_le_bytes([size[0], size[1]]),
                rows: u16::from_le_bytes([size[2], size[3]]),
            },
            backlog: Bytes::copy_from_slice(backlog),
        })
    }

    pub fn stop_if(&self, id: K, stop: impl FnOnce(&Keys<'_>) -> bool) -> bool {
        let Some(pty) = self.pty(id) else {
            return false;
        };
        let mut host = pty.host.lock().unwrap();
        let stopping = stop(&Keys(Some(&pty)));
        if stopping {
            let _ = wire::write(&mut *host, &Frame::Kill);
        }
        stopping
    }

    pub fn stop(&self, id: K) {
        if let Some(pty) = self.pty(id) {
            pty.send(&Frame::Kill);
        }
    }

    pub fn guard(&self, id: K, caller: u32) {
        let Some(pty) = self.pty(id) else { return };
        let root = *pty.pid.lock().unwrap();
        let Some(shell) = root.and_then(|root| Snapshot::now().child_on_path(root, caller)) else {
            return;
        };
        let mut shells = pty.guarded.lock().unwrap();
        if !shells.contains(&shell) {
            shells.push(shell);
        }
        process_tree::alive(&mut shells);
    }

    pub fn stop_guarded(&self, id: K) -> usize {
        let Some(pty) = self.pty(id) else { return 0 };
        let shells = std::mem::take(&mut *pty.guarded.lock().unwrap());
        let Some(root) = pty.pid.lock().unwrap().filter(|_| !shells.is_empty()) else {
            return 0;
        };
        let snapshot = Snapshot::now();
        snapshot.kill_trees(
            shells
                .into_iter()
                .filter(|&shell| snapshot.descends(shell, root)),
        )
    }

    pub fn pid(&self, id: K) -> Option<u32> {
        *self.pty(id)?.pid.lock().unwrap()
    }

    fn pty(&self, id: K) -> Option<Arc<Pty>> {
        self.nodes.read().unwrap().get(&id).cloned()
    }

    fn file(&self, id: K) -> PathBuf {
        self.dir.join(id.to_string())
    }

    fn kept_file(&self, id: K) -> PathBuf {
        self.kept.join(id.to_string())
    }
}

fn start_host(exe: &std::path::Path, spec: &host::Spec) -> Result<String, String> {
    let mut host = std::process::Command::new(exe);
    host.arg("pty-host");
    let spec = serde_json::to_vec(spec).map_err(|e| e.to_string())?;
    match process_tree::announced(host, &spec) {
        Ok(address) if address.is_empty() => {
            Err("pty-host: exited before announcing its address".into())
        }
        Ok(address) => Ok(address),
        Err(e) => Err(format!("pty-host: {e}")),
    }
}

pub fn start_in_distro(start: distro::Start) -> Result<SocketAddr, String> {
    let (program, cmd, setup) = distro_commands(start);
    let address = start_host(Path::new(&program), &host::Spec::of(&cmd, setup.as_ref()))?;
    address
        .parse()
        .map_err(|e| format!("pty-host: {address}: {e}"))
}

fn distro_commands(start: distro::Start) -> (String, CommandBuilder, Option<CommandBuilder>) {
    let distro::Start {
        program,
        command,
        setup,
    } = start;
    let build = |argv: Vec<String>| {
        let mut cmd = CommandBuilder::from_argv(argv.into_iter().map(Into::into).collect());
        cmd.cwd(&command.cwd);
        for (key, value) in &command.env {
            cmd.env(key, value);
        }
        cmd
    };
    let cmd = build(command.argv.clone());
    let setup = setup.map(build);
    (program, cmd, setup)
}

pub struct Keys<'a>(Option<&'a Pty>);

impl Keys<'_> {
    pub fn screen(&self) -> Screen {
        self.0.map_or_else(Screen::default, Pty::screen)
    }
}

impl Pty {
    fn screen(&self) -> Screen {
        let PtySize { cols, rows } = *self.size.borrow();
        let bytes = self.output.lock().unwrap().backlog.to_vec();
        let size = |n: u16| NonZeroU16::new(n.max(1)).expect("1 이상이다");
        let mut parser = vt100::Parser::new(size(rows), size(cols), 0);
        parser.process(&bytes);
        let screen = parser.screen();
        let (row, col) = screen.cursor_position();
        let above = screen.rows(0, cols).take(row.into());
        let at = screen.rows(0, col).nth(row.into());
        Screen {
            text: screen.contents(),
            undimmed: undimmed(screen),
            to_cursor: above.chain(at).collect::<Vec<_>>().join("\n"),
            cols,
        }
    }

    fn send(&self, frame: &Frame) {
        let _ = wire::write(&mut *self.host.lock().unwrap(), frame);
    }

    fn change_size(&self, change: impl FnOnce(&mut Sizing, PtySize) -> Option<PtySize>) {
        let mut host = self.host.lock().unwrap();
        let next = {
            let mut sizing = self.sizing.lock().unwrap();
            let Some(next) = change(&mut sizing, *self.size.borrow()) else {
                return;
            };
            let changed = self
                .size
                .send_if_modified(|size| std::mem::replace(size, next) != next);
            let first = sizing.host == HostSize::Unreported;
            if first {
                sizing.host = HostSize::Claimed;
            }
            (changed || first).then_some(next)
        };
        if let Some(PtySize { cols, rows }) = next {
            let _ = wire::write(&mut *host, &Frame::Resize { cols, rows });
        }
    }
}

fn undimmed(screen: &vt100::Screen) -> String {
    let (rows, cols) = screen.size();
    let (rows, cols) = (rows.get(), cols.get());
    let line = |row| {
        let cells = (0..cols).filter_map(|col| screen.cell(row, col));
        let text: String = cells
            .filter(|cell| !cell.is_wide_continuation())
            .map(|cell| match cell.dim() || !cell.has_contents() {
                true => " ",
                false => cell.contents(),
            })
            .collect();
        text.trim_end().to_owned()
    };
    (0..rows).map(line).collect::<Vec<_>>().join("\n")
}

impl Output {
    fn settled(&self, now: Instant) -> bool {
        now.duration_since(self.last_push) >= SETTLED_AFTER
    }

    fn push(&mut self, chunk: &[u8]) {
        self.backlog.push(chunk);
        self.last_push = Instant::now();
        let _ = self.tx.send(Bytes::copy_from_slice(chunk));
    }
}

impl Backlog {
    fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend(chunk);
        let dropped = self.bytes.len().saturating_sub(BACKLOG_BYTES);
        if dropped == 0 {
            return;
        }
        let head: Vec<u8> = self.bytes.range(..dropped).copied().collect();
        let cut = cut_sequence(&head, self.bytes.range(dropped..).copied());
        self.remember_modes(&[head, cut].concat());
        self.bytes.drain(..dropped);
    }

    fn remember_modes(&mut self, bytes: &[u8]) {
        for event in mode_events(bytes) {
            match event {
                ModeEvent::Set(mode, on) if KEPT_MODES.contains(&mode) => {
                    self.modes.retain(|(m, _)| *m != mode);
                    self.modes.push((mode, on));
                }
                ModeEvent::Set(..) => {}
                ModeEvent::Reset => self.modes.clear(),
            }
        }
    }

    fn to_vec(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.bytes.len());
        for (mode, on) in &self.modes {
            out.extend(format!("\x1b[?{mode}{}", if *on { 'h' } else { 'l' }).bytes());
        }
        out.extend(&self.bytes);
        out
    }
}

fn cut_sequence(head: &[u8], rest: impl Iterator<Item = u8>) -> Vec<u8> {
    let Some(esc) = head.iter().rposition(|b| *b == 0x1b) else {
        return Vec::new();
    };
    let mut seq = head[esc..].to_vec();
    let start = seq.len();
    if !open_sequence(&seq) {
        return Vec::new();
    }
    for b in rest {
        seq.push(b);
        let open = match seq.len() {
            ..=3 => open_sequence(&seq),
            _ => b.is_ascii_digit() || b == b';',
        };
        if !open {
            break;
        }
    }
    seq.split_off(start)
}

fn open_sequence(seq: &[u8]) -> bool {
    match seq {
        [0x1b] | [0x1b, b'['] => true,
        [0x1b, b'[', b'?', params @ ..] => params.iter().all(|b| b.is_ascii_digit() || *b == b';'),
        _ => false,
    }
}

enum ModeEvent {
    Set(u16, bool),
    Reset,
}

fn mode_events(bytes: &[u8]) -> Vec<ModeEvent> {
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(start) = bytes[at..].iter().position(|b| *b == 0x1b) {
        let esc = at + start;
        if bytes.get(esc + 1) == Some(&b'c') {
            found.push(ModeEvent::Reset);
            at = esc + 2;
            continue;
        }
        if bytes.get(esc + 1..esc + 3) != Some(b"[?") {
            at = esc + 1;
            continue;
        }
        let params = esc + 3;
        let len = bytes[params..]
            .iter()
            .take_while(|b| b.is_ascii_digit() || **b == b';')
            .count();
        at = params + len;
        let on = match bytes.get(at) {
            Some(b'h') => true,
            Some(b'l') => false,
            _ => continue,
        };
        for param in bytes[params..at].split(|b| *b == b';') {
            if let Some(mode) = std::str::from_utf8(param).ok().and_then(|p| p.parse().ok()) {
                found.push(ModeEvent::Set(mode, on));
            }
        }
    }
    found
}

pub fn typed(input: &[u8]) -> bool {
    let mut rest = input;
    while !rest.is_empty() {
        let Some(len) = report_len(rest) else {
            return true;
        };
        rest = &rest[len..];
    }
    false
}

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

fn report_len(seq: &[u8]) -> Option<usize> {
    if seq.first() != Some(&ESC) {
        return None;
    }
    match seq.get(1)? {
        b']' | b'P' | b'_' => {
            let end = seq[2..]
                .iter()
                .enumerate()
                .find(|&(i, &b)| b == BEL || (b == ESC && seq.get(i + 3) == Some(&b'\\')))?;
            Some(2 + end.0 + if *end.1 == BEL { 1 } else { 2 })
        }
        b'[' => {
            let body = &seq[2..];
            if body.first() == Some(&b'M') {
                return (body.len() >= 4).then_some(6);
            }
            let end = body.iter().position(|b| (0x40..=0x7e).contains(b))?;
            let (params, last) = (&body[..end], body[end]);
            let prefixed = |p: u8| params.first() == Some(&p);
            let report = match last {
                b'I' | b'O' => params.is_empty(),
                b'R' | b'n' | b't' => true,
                b'c' => prefixed(b'?') || prefixed(b'>'),
                b'y' => params.ends_with(b"$"),
                b'u' => prefixed(b'?'),
                b'M' | b'm' => prefixed(b'<'),
                _ => false,
            };
            report.then_some(2 + end + 1)
        }
        _ => None,
    }
}

#[cfg(all(test, unix))]
pub fn old_in_process(dir: &std::path::Path, id: impl Display, screen: &[u8]) {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join(id.to_string()),
        listener.local_addr().unwrap().to_string(),
    )
    .unwrap();
    let screen = screen.to_vec();
    std::thread::spawn(move || {
        for daemon in listener.incoming() {
            let Ok(mut daemon) = daemon else { return };
            let screen = screen.clone();
            std::thread::spawn(move || {
                if !matches!(wire::read(&mut daemon), Ok(Frame::Hello { .. })) {
                    return;
                }
                let greeted = wire::write(
                    &mut daemon,
                    &Frame::Hello {
                        version: 3,
                        pid: None,
                    },
                )
                .and_then(|()| wire::write(&mut daemon, &Frame::Resize { cols: 80, rows: 24 }))
                .and_then(|()| wire::write(&mut daemon, &Frame::Data(screen)));
                while greeted.is_ok() && wire::read(&mut daemon).is_ok() {}
            });
        }
    });
}

#[cfg(test)]
pub fn in_process(dir: &std::path::Path, id: impl Display, cmd: CommandBuilder) {
    let started = host::start(cmd, None, None).unwrap();
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(id.to_string()), started.address.to_string()).unwrap();
    std::thread::spawn(move || started.wait());
}

#[cfg(test)]
mod endpoint_tests;

#[cfg(test)]
mod distro_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dim_text_like_an_input_placeholder_is_not_in_the_undimmed_screen() {
        let size = |n| NonZeroU16::new(n).unwrap();
        let mut parser = vt100::Parser::new(size(40), size(120), 0);
        parser.process(include_bytes!("../profile/screens/claude-idle.txt"));
        let screen = parser.screen();
        let input = |text: String| text.lines().find(|l| l.starts_with('❯')).map(str::to_owned);
        assert_eq!(
            input(screen.contents()).as_deref(),
            Some("❯\u{a0}Try \"write a test for styles.css\"")
        );
        assert_eq!(input(undimmed(screen)).as_deref(), Some("❯"));
        assert!(undimmed(screen).contains("⏵⏵ auto mode on"));
    }

    #[cfg(unix)]
    fn read_until(attached: &mut Attached, text: &str) -> String {
        let mut seen = String::from_utf8_lossy(&attached.backlog).into_owned();
        while !seen.contains(text) {
            let chunk = attached.output.blocking_recv().unwrap();
            seen.push_str(&String::from_utf8_lossy(&chunk));
        }
        seen
    }

    #[cfg(unix)]
    #[test]
    fn terminal_output_is_available_before_exit_callback() {
        for (script, expected) in [("exit 0", false), ("printf 'review this'; exit 0", true)] {
            let dir = tempfile::tempdir().unwrap();
            let mut cmd = CommandBuilder::new("sh");
            cmd.args(["-c", script]);
            in_process(dir.path(), 1, cmd);
            let nodes = Arc::new(NodeHost::new(dir.path().to_path_buf()));
            let observed = nodes.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            nodes
                .resume(1, move |code| {
                    tx.send((code, observed.has_output(1))).unwrap();
                })
                .unwrap();
            assert_eq!(
                rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(),
                (Exit::Command(0), expected)
            );
            assert!(nodes.has_output(99));
            nodes.kill(1);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_borrowed_size_goes_back_unless_another_screen_claimed() {
        let dir = tempfile::tempdir().unwrap();
        let mut cmd = CommandBuilder::new("sh");
        cmd.args(["-c", "read line"]);
        in_process(dir.path(), 1, cmd);
        let nodes = NodeHost::new(dir.path().to_path_buf());
        nodes.resume(1, |_| {}).unwrap();
        let size = nodes.attach(1).unwrap().size;
        let now = || *size.borrow();
        let pc = PtySize {
            cols: 120,
            rows: 40,
        };
        let phone = PtySize { cols: 50, rows: 30 };

        nodes.resize(1, pc.cols, pc.rows);
        let first = Borrower::fresh();
        nodes.borrow(1, first, phone.cols, phone.rows);
        assert_eq!(now(), phone);
        nodes.borrow(1, first, 52, 31);
        nodes.give_back(1, first);
        assert_eq!(now(), pc, "phone closed: the PC size comes back");

        let second = Borrower::fresh();
        nodes.borrow(1, second, phone.cols, phone.rows);
        nodes.resize(1, 100, 30);
        nodes.give_back(1, second);
        assert_eq!(
            now(),
            PtySize {
                cols: 100,
                rows: 30
            },
            "the PC claimed after the phone"
        );

        let (a, b) = (Borrower::fresh(), Borrower::fresh());
        nodes.borrow(1, a, phone.cols, phone.rows);
        nodes.borrow(1, b, 60, 20);
        nodes.give_back(1, a);
        assert_eq!(
            now(),
            PtySize { cols: 60, rows: 20 },
            "a phone that lost the size gives nothing back"
        );
        nodes.give_back(1, b);
        assert_eq!(
            now(),
            PtySize {
                cols: 100,
                rows: 30
            }
        );
        nodes.kill(1);
    }

    #[cfg(unix)]
    #[test]
    fn a_new_daemon_reattaches_with_earlier_output() {
        let dir = tempfile::tempdir().unwrap();
        let mut cmd = CommandBuilder::new("sh");
        cmd.args(["-c", "echo before; read line; echo got $line; exit 3"]);
        in_process(dir.path(), 1, cmd);
        let first = NodeHost::new(dir.path().to_path_buf());
        first.resume(1, |_| {}).unwrap();
        read_until(&mut first.attach(1).unwrap(), "before");
        let first_pid = *first.pty(1).unwrap().pid.lock().unwrap();

        let second = NodeHost::new(dir.path().to_path_buf());
        let (tx, rx) = std::sync::mpsc::channel();
        second
            .resume(1, move |code| tx.send(code).unwrap())
            .unwrap();
        assert_eq!(*second.pty(1).unwrap().pid.lock().unwrap(), first_pid);
        let mut attached = second.attach(1).unwrap();
        second
            .write_if(1, |_| Ok::<_, std::convert::Infallible>(b"x\n".to_vec()))
            .unwrap();
        assert!(read_until(&mut attached, "got x").contains("before"));
        assert_eq!(rx.recv().unwrap(), Exit::Command(3));
        assert!(!dir.path().join("1").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_kept_screen_outlives_the_daemon_until_a_new_pty_or_removal() {
        let dir = tempfile::tempdir().unwrap();
        let hosts = dir.path().join("nodes");
        let mut old = CommandBuilder::new("sh");
        old.args([
            "-c",
            "printf '\\033[?1049h'; head -c 300000 /dev/zero | tr '\\0' x; printf last; read line",
        ]);
        in_process(&hosts, 1, old);
        let first = Arc::new(NodeHost::new(hosts.clone()));
        let keeper = first.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        first
            .resume(1, move |_| tx.send(keeper.keep(1).is_ok()).unwrap())
            .unwrap();
        first.resize(1, 90, 30);
        read_until(&mut first.attach(1).unwrap(), "last");
        first.stop(1);
        assert!(rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap());

        let second = NodeHost::new(hosts.clone());
        assert!(second.attach(1).is_none());
        let kept = second.kept(1).unwrap();
        assert_eq!(kept.size, PtySize { cols: 90, rows: 30 });
        assert!(kept.backlog.starts_with(b"\x1b[?1049h"));
        assert!(kept.backlog.ends_with(b"last"));
        let saved = std::fs::metadata(hosts.with_extension("screens").join("1")).unwrap();
        assert!(saved.len() <= (4 + b"\x1b[?1049h".len() + BACKLOG_BYTES) as u64);

        let mut new = CommandBuilder::new("sh");
        new.args(["-c", "echo new; read line"]);
        in_process(&hosts, 1, new);
        second.resume(1, |_| {}).unwrap();
        assert!(second.kept(1).is_none());
        read_until(&mut second.attach(1).unwrap(), "new");
        second.keep(1).unwrap();
        let replaced = String::from_utf8_lossy(&second.kept(1).unwrap().backlog).into_owned();
        assert!(replaced.contains("new") && !replaced.contains("last"));

        second.kill(1);
        assert!(second.kept(1).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_stopped_pty_stays_until_the_new_process_replaces_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut old = CommandBuilder::new("sh");
        old.args(["-c", "echo old; read line"]);
        in_process(dir.path(), 1, old);
        let nodes = NodeHost::new(dir.path().to_path_buf());
        let (tx, rx) = std::sync::mpsc::channel();
        nodes.resume(1, move |code| tx.send(code).unwrap()).unwrap();
        let mut viewer = nodes.attach(1).unwrap();
        read_until(&mut viewer, "old");

        nodes.stop(1);
        rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        let between = nodes
            .attach(1)
            .expect("the screen still attaches between the two processes");
        assert!(String::from_utf8_lossy(&between.backlog).contains("old"));

        let mut new = CommandBuilder::new("sh");
        new.args(["-c", "echo new; read line"]);
        in_process(dir.path(), 1, new);
        nodes.resume(1, |_| {}).unwrap();
        while let Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) =
            viewer.output.blocking_recv()
        {}
        read_until(&mut nodes.attach(1).unwrap(), "new");
        nodes.kill(1);
    }

    #[cfg(unix)]
    #[test]
    fn a_new_daemon_attaches_while_the_old_connection_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let mut cmd = CommandBuilder::new("sh");
        cmd.args(["-c", "exec yes noise"]);
        in_process(dir.path(), 1, cmd);
        let address = std::fs::read_to_string(dir.path().join("1")).unwrap();
        let mut old = TcpStream::connect(address.trim()).unwrap();
        wire::greet(&mut old).unwrap();
        std::thread::sleep(Duration::from_secs(3));

        let second = NodeHost::new(dir.path().to_path_buf());
        second.resume(1, |_| {}).unwrap();
        let mut attached = second.attach(1).unwrap();
        let chunk = attached.output.blocking_recv().unwrap();
        assert!(String::from_utf8_lossy(&chunk).contains("noise"));
        second.kill(1);
        drop(old);
    }

    #[cfg(unix)]
    #[test]
    fn setup_runs_first_and_gates_the_command() {
        let sh = |script: &str| {
            let mut cmd = CommandBuilder::new("sh");
            cmd.args(["-c", script]);
            cmd
        };
        for (setup, exit, has_agent) in [
            ("echo setup-ok", Exit::Command(4), true),
            (
                "printf 'progress 1\\rprogress 2\\n\\033[31msetup-bad\\033[0m\\n'; exit 3",
                Exit::Setup(3),
                false,
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let started = host::start(sh("echo agent $$; exit 4"), Some(sh(setup)), None).unwrap();
            std::fs::write(dir.path().join("1"), started.address.to_string()).unwrap();
            std::thread::spawn(move || started.wait());
            let nodes = NodeHost::new(dir.path().to_path_buf());
            let (tx, rx) = std::sync::mpsc::channel();
            nodes.resume(1, move |code| tx.send(code).unwrap()).unwrap();
            let mut attached = nodes.attach(1).unwrap();
            let code = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let mut seen = String::from_utf8_lossy(&attached.backlog).into_owned();
            while let Ok(chunk) = attached.output.try_recv() {
                seen.push_str(&String::from_utf8_lossy(&chunk));
            }
            assert_eq!(code, exit, "{seen}");
            assert!(seen.contains("setup-"), "{seen}");
            assert_eq!(seen.contains("agent "), has_agent, "{seen}");
            if !has_agent {
                assert_eq!(nodes.tail(1), "progress 2\nsetup-bad");
            }
            if has_agent {
                let agent: u32 = seen
                    .split("agent ")
                    .nth(1)
                    .unwrap()
                    .split_whitespace()
                    .next()
                    .unwrap()
                    .parse()
                    .unwrap();
                assert_eq!(*nodes.pty(1).unwrap().pid.lock().unwrap(), Some(agent));
            }
        }
    }

    #[test]
    fn a_late_host_size_during_a_blocked_write_does_not_deadlock() {
        const FRAMES: usize = 16;
        const FRAME: usize = 1 << 20;
        let limit = Duration::from_secs(20);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("1"),
            listener.local_addr().unwrap().to_string(),
        )
        .unwrap();
        let (writing_tx, writing) = std::sync::mpsc::channel::<()>();
        let (host_tx, host_done) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            wire::welcome(&mut stream, None).unwrap();
            writing.recv().unwrap();
            wire::write(&mut stream, &Frame::Resize { cols: 80, rows: 24 }).unwrap();
            for _ in 0..FRAMES {
                wire::write(&mut stream, &Frame::Data(vec![b'o'; FRAME])).unwrap();
            }
            let (mut sizes, mut input) = (Vec::new(), 0);
            while input < FRAMES * FRAME {
                match wire::read(&mut stream).unwrap() {
                    Frame::Data(bytes) => input += bytes.len(),
                    Frame::Resize { cols, rows } => sizes.push(PtySize { cols, rows }),
                    _ => {}
                }
            }
            host_tx.send(sizes).unwrap();
        });
        let nodes = Arc::new(NodeHost::new(dir.path().to_path_buf()));
        nodes.resume(1, |_| {}).unwrap();
        let claimed = PtySize {
            cols: 100,
            rows: 40,
        };
        nodes.resize(1, claimed.cols, claimed.rows);
        let (writer_tx, writer_done) = std::sync::mpsc::channel();
        let writer = nodes.clone();
        std::thread::spawn(move || {
            for _ in 0..FRAMES {
                writer
                    .write_if(1, |_| Ok::<_, std::convert::Infallible>(vec![b'i'; FRAME]))
                    .unwrap();
            }
            writer_tx.send(()).unwrap();
        });
        std::thread::sleep(Duration::from_millis(300));
        writing_tx.send(()).unwrap();
        writer_done
            .recv_timeout(limit)
            .expect("the daemon write finished");
        let sizes = host_done
            .recv_timeout(limit)
            .expect("the host read all input");
        assert_eq!(
            sizes.last(),
            Some(&claimed),
            "the host keeps the claimed size"
        );
        assert_eq!(*nodes.attach(1).unwrap().size.borrow(), claimed);
    }

    #[test]
    fn a_version_1_host_still_works() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("1"),
            listener.local_addr().unwrap().to_string(),
        )
        .unwrap();
        let host = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            assert!(matches!(
                wire::read(&mut stream).unwrap(),
                Frame::Hello { .. }
            ));
            let hello = Frame::Hello {
                version: 1,
                pid: Some(42),
            };
            wire::write(&mut stream, &hello).unwrap();
            wire::write(&mut stream, &Frame::Data(b"old host".to_vec())).unwrap();
            assert_eq!(wire::read(&mut stream).unwrap(), Frame::Data(b"x".to_vec()));
            wire::write(&mut stream, &Frame::Exit(5)).unwrap();
        });
        let nodes = NodeHost::new(dir.path().to_path_buf());
        let (tx, rx) = std::sync::mpsc::channel();
        nodes.resume(1, move |code| tx.send(code).unwrap()).unwrap();
        assert_eq!(*nodes.pty(1).unwrap().pid.lock().unwrap(), Some(42));
        nodes
            .write_if(1, |_| Ok::<_, std::convert::Infallible>(b"x".to_vec()))
            .unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Exit::Command(5)
        );
        let attached = nodes.attach(1).unwrap();
        assert_eq!(&attached.backlog[..], b"old host");
        host.join().unwrap();
    }

    #[test]
    fn only_a_refused_connection_means_the_host_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("1"), "127.0.0.1:1").unwrap();
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        std::fs::write(
            dir.path().join("2"),
            silent.local_addr().unwrap().to_string(),
        )
        .unwrap();
        let nodes = NodeHost::new(dir.path().to_path_buf());
        assert_eq!(nodes.resume(1, |_| {}), Err(Unattached::Gone));
        assert!(matches!(
            nodes.resume(2, |_| {}),
            Err(Unattached::Silent(_))
        ));
        drop(silent);
    }

    #[test]
    fn a_dropped_connection_is_lost_not_an_exit() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("1"),
            listener.local_addr().unwrap().to_string(),
        )
        .unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            wire::welcome(&mut stream, Some(42)).unwrap();
        });
        let nodes = NodeHost::new(dir.path().to_path_buf());
        let (tx, rx) = std::sync::mpsc::channel();
        nodes.resume(1, move |exit| tx.send(exit).unwrap()).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Exit::Lost);
    }

    #[cfg(unix)]
    #[test]
    fn stop_guarded_ends_the_shell_and_its_children() {
        let dir = tempfile::tempdir().unwrap();
        let mut cmd = CommandBuilder::new("sh");
        cmd.args(["-c", "sh -c 'echo $$; sleep 30 & sleep 30' ; sleep 30"]);
        in_process(dir.path(), 1, cmd);
        let host = NodeHost::new(dir.path().to_path_buf());
        let (tx, rx) = std::sync::mpsc::channel();
        host.resume(1, move |code| tx.send(code).unwrap()).unwrap();
        let text = read_until(&mut host.attach(1).unwrap(), "\n");
        let shell: u32 = text.trim().parse().unwrap();
        host.guard(1, std::process::id());
        host.guard(1, shell);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(host.stop_guarded(1), 3);
        assert_eq!(host.stop_guarded(1), 0);
        host.kill(1);
        rx.recv().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn spec_carries_argv_cwd_and_the_whole_environment() {
        let mut cmd = CommandBuilder::new("sh");
        cmd.args(["-c", "echo $LOOM_TOKEN"]);
        cmd.cwd("/tmp");
        cmd.env("LOOM_TOKEN", "t1");
        cmd.env_remove("HOME");
        let json = serde_json::to_string(&host::Spec::of(&cmd, None)).unwrap();
        let back: host::Spec = serde_json::from_str(&json).unwrap();
        let (back, setup) = back.commands();
        assert!(setup.is_none());
        assert_eq!(back.get_argv(), cmd.get_argv());
        assert_eq!(back.get_cwd().unwrap(), "/tmp");
        assert_eq!(back.get_env("LOOM_TOKEN").unwrap(), "t1");
        assert_eq!(back.get_env("HOME"), None);
        assert_eq!(
            back.iter_full_env_as_str().count(),
            cmd.iter_full_env_as_str().count()
        );
    }

    #[test]
    fn output_settles_after_quiet_time() {
        let mut output = Output {
            backlog: Backlog::default(),
            tx: broadcast::channel(1).0,
            last_push: Instant::now(),
        };
        output.push(b"\x1b]11;?\x1b\\");
        let pushed = output.last_push;
        assert!(
            !output.settled(pushed),
            "방금 보낸 질의는 아직 기다릴 수 있다"
        );
        assert!(!output.settled(pushed + SETTLED_AFTER - Duration::from_millis(1)));
        assert!(
            output.settled(pushed + SETTLED_AFTER),
            "조용해진 뒤의 질의는 시한이 지났다"
        );
    }

    #[test]
    fn backlog_keeps_only_the_newest_bytes() {
        let mut backlog = Backlog::default();
        backlog.push(&vec![b'a'; BACKLOG_BYTES]);
        backlog.push(b"xyz");
        let bytes = backlog.to_vec();
        assert_eq!(bytes.len(), BACKLOG_BYTES);
        assert_eq!(&bytes[BACKLOG_BYTES - 4..], b"axyz");
    }

    fn replayed_modes(stream: &[u8]) -> (String, vt100::Parser) {
        let mut backlog = Backlog::default();
        backlog.push(stream);
        backlog.push(&vec![b'a'; BACKLOG_BYTES]);
        let bytes = backlog.to_vec();
        let prefix = String::from_utf8_lossy(&bytes[..bytes.len() - BACKLOG_BYTES]).into_owned();
        let mut parser = vt100::Parser::new(
            NonZeroU16::new(24).unwrap(),
            NonZeroU16::new(80).unwrap(),
            0,
        );
        parser.process(&bytes);
        (prefix, parser)
    }

    #[test]
    fn backlog_keeps_terminal_modes_after_it_drops_their_bytes() {
        let (prefix, parser) =
            replayed_modes(b"\x1b[?1049h\x1b[?7l\x1b[?1003h\x1b[?1003l\x1b[?1002;1006h");
        assert_eq!(prefix, "\x1b[?1049h\x1b[?1003l\x1b[?1002h\x1b[?1006h");
        assert!(parser.screen().alternate_screen());
        assert_eq!(
            parser.screen().mouse_protocol_mode(),
            vt100::MouseProtocolMode::ButtonMotion
        );
    }

    #[test]
    fn backlog_replays_modes_in_the_order_of_their_last_change() {
        let (prefix, parser) = replayed_modes(b"\x1b[?1049h\x1b[?1049l\x1b[?47h");
        assert_eq!(prefix, "\x1b[?1049l\x1b[?47h");
        assert!(parser.screen().alternate_screen());
        let (prefix, _) = replayed_modes(b"\x1b[?1002h\x1b[?1000h\x1b[?1002l");
        assert_eq!(prefix, "\x1b[?1000h\x1b[?1002l");
    }

    #[test]
    fn backlog_forgets_modes_after_a_terminal_reset() {
        let (prefix, parser) = replayed_modes(b"\x1b[?1000h\x1b[?1006h\x1bc\x1b[?2004h");
        assert_eq!(prefix, "\x1b[?2004h");
        assert_eq!(
            parser.screen().mouse_protocol_mode(),
            vt100::MouseProtocolMode::None
        );
    }

    #[test]
    fn backlog_reads_a_mode_sequence_cut_at_the_drop_edge() {
        let mut backlog = Backlog::default();
        backlog.push(&vec![b'a'; BACKLOG_BYTES - 4]);
        backlog.push(b"\x1b[?1006h");
        backlog.push(&vec![b'b'; BACKLOG_BYTES - 4]);
        assert!(backlog.to_vec().starts_with(b"\x1b[?1006h"));
    }

    #[test]
    fn backlog_reads_digits_after_an_open_mode_sequence_in_linear_time() {
        let mut backlog = Backlog::default();
        backlog.push(b"\x1b[?");
        backlog.push(&vec![b'1'; BACKLOG_BYTES - 3]);
        let started = Instant::now();
        backlog.push(b"x");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(backlog.modes.is_empty());
    }

    #[test]
    fn backlog_reads_a_long_mode_sequence_when_only_its_escape_drops() {
        let seq = b"\x1b[?1049;1003;1006;1004;2004;1000;1002;1003h";
        let mut backlog = Backlog::default();
        backlog.push(seq);
        backlog.push(&vec![b'a'; BACKLOG_BYTES - seq.len()]);
        backlog.push(b"b");
        let bytes = backlog.to_vec();
        let mut parser = vt100::Parser::new(
            NonZeroU16::new(24).unwrap(),
            NonZeroU16::new(80).unwrap(),
            0,
        );
        parser.process(&bytes);
        assert!(parser.screen().alternate_screen());
        assert_eq!(
            parser.screen().mouse_protocol_mode(),
            vt100::MouseProtocolMode::AnyMotion
        );
        backlog.push(&vec![b'c'; BACKLOG_BYTES]);
        for cut in 1..seq.len() {
            let mut edge = Backlog::default();
            edge.push(seq);
            edge.push(&vec![b'a'; BACKLOG_BYTES - seq.len()]);
            edge.push(&vec![b'b'; cut]);
            edge.push(&vec![b'c'; BACKLOG_BYTES]);
            let mut parser = vt100::Parser::new(
                NonZeroU16::new(24).unwrap(),
                NonZeroU16::new(80).unwrap(),
                0,
            );
            parser.process(&edge.to_vec());
            assert!(parser.screen().alternate_screen(), "cut {cut}");
        }
        assert_eq!(
            String::from_utf8_lossy(&backlog.to_vec()[..backlog.to_vec().len() - BACKLOG_BYTES]),
            "\x1b[?1049h\x1b[?1006h\x1b[?1004h\x1b[?2004h\x1b[?1000h\x1b[?1002h\x1b[?1003h"
        );
    }

    #[test]
    fn keys_are_typed_but_terminal_replies_are_not() {
        for key in [
            &b"y"[..],
            b"\r",
            b"\x1b",
            b"\x1b[A",
            b"\x1bOB",
            b"\x1b[97u",
            b"\x1b[200~hi\x1b[201~",
            b"\x1b[I1",
        ] {
            assert!(typed(key), "{key:?}");
        }
        for reply in [
            &b"\x1b[I"[..],
            b"\x1b[O",
            b"\x1b[12;40R",
            b"\x1b[?1;2c",
            b"\x1b[>0;276;0c",
            b"\x1b[0n",
            b"\x1b[?2004;1$y",
            b"\x1b[?7u",
            b"\x1b[<0;10;5M\x1b[<0;10;5m",
            b"\x1b]11;rgb:0000/0000/0000\x1b\\",
            b"\x1b]10;rgb:ffff/ffff/ffff\x07",
        ] {
            assert!(!typed(reply), "{reply:?}");
        }
    }
}
