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
    Unreached(String),
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
    Gone,
}

impl Exit {
    fn frame(self) -> Frame {
        match self {
            Exit::Command(code) => Frame::Exit(code),
            Exit::Setup(code) => Frame::SetupFailed(code),
            Exit::Lost | Exit::Gone => Frame::Exit(LOST_CODE),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Unattached {
    Gone,
    Unreached(String),
    Failed(String),
}

impl Display for Unattached {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unattached::Gone => f.write_str("pty-host is gone"),
            Unattached::Unreached(cause) | Unattached::Failed(cause) => f.write_str(cause),
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
    kept: PathBuf,
    nodes: Arc<RwLock<HashMap<K, Arc<Pty>>>>,
    links: Option<Links<K>>,
    distros: Option<Arc<Distros>>,
}

type OnExit = Box<dyn FnOnce(Exit) + Send>;

#[derive(Clone)]
pub enum Address {
    Here(SocketAddr),
    Distro { at: SocketAddr, distro: String },
}

impl Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Address::Here(at) | Address::Distro { at, .. } => write!(f, "{at}"),
        }
    }
}

impl Address {
    pub fn parse(text: &str, distro: Option<String>) -> Result<Self, String> {
        let at = text.lines().next().unwrap_or_default().trim();
        let at = at
            .parse()
            .map_err(|e| format!("pty-host address {at}: {e}"))?;
        Ok(match distro {
            Some(distro) => Address::Distro { at, distro },
            None => Address::Here(at),
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
    pub fn new(kept: PathBuf) -> Self {
        Self {
            kept,
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

    pub fn start(
        &self,
        cmd: &CommandBuilder,
        setup: Option<&CommandBuilder>,
    ) -> Result<Address, String> {
        start_host(&process_tree::own_exe()?, &host::Spec::of(cmd, setup)).map(Address::Here)
    }

    pub fn start_in(&self, distro: &str, start: distro::Start) -> Result<Address, String> {
        let distros = self.distros.as_ref().ok_or("no WSL distro host")?;
        Ok(Address::Distro {
            at: distros.start(distro, start)?,
            distro: distro.to_owned(),
        })
    }

    pub fn spawn_while_linked(
        &self,
        id: K,
        cmd: &CommandBuilder,
        on_exit: impl FnOnce(Exit) + Send + 'static,
    ) -> Result<(), String> {
        let spec = host::Spec::of(cmd, None).while_linked();
        let at = start_host(&process_tree::own_exe()?, &spec)?;
        self.resume(id, &Address::Here(at), on_exit)
            .map_err(|e| e.to_string())
    }

    pub fn resume(
        &self,
        id: K,
        at: &Address,
        on_exit: impl FnOnce(Exit) + Send + 'static,
    ) -> Result<(), Unattached> {
        let _ = std::fs::remove_file(self.kept_file(id));
        self.link(id, at.clone(), Box::new(on_exit), None)
            .map_err(|(unattached, _)| unattached)
    }

    pub fn end_at(&self, at: &Address) {
        if let Ok((mut host, _)) = self.connect(at, wire::greet) {
            let _ = wire::write(&mut host, &Frame::Kill);
        }
    }

    fn link(
        &self,
        id: K,
        at: Address,
        on_exit: OnExit,
        replacing: Option<Arc<Pty>>,
    ) -> Result<(), (Unattached, OnExit)> {
        let (mut reader, greeted) = match self.connect(&at, wire::greet) {
            Ok(found) => found,
            Err(e) => return Err((e, on_exit)),
        };
        let pid = greeted.pid;
        let linked = match greeted.version >= wire::STREAMS_SINCE && self.links.is_some() {
            true => match self.connect(&at, |link| wire::greet_streams(link, greeted.version)) {
                Ok((link, held)) => Linked::Streams { link, held },
                Err(e) => {
                    let _ = wire::write(&mut reader, &Frame::Kill);
                    std::thread::spawn(move || while wire::read(&mut reader).is_ok() {});
                    return Err((
                        Unattached::Failed(format!("node endpoint link: {e}")),
                        on_exit,
                    ));
                }
            },
            false => Linked::Terminal,
        };
        let host = match reader.try_clone() {
            Ok(host) => host,
            Err(e) => return Err((Unattached::Failed(e.to_string()), on_exit)),
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
            let (on_exit, exit) = match exit {
                Some(exit) => (on_exit, exit),
                None => match host.link(id, at, on_exit, Some(pty.clone())) {
                    Ok(()) => return,
                    Err((Unattached::Unreached(cause), _)) => {
                        let mut nodes = host.nodes.write().unwrap();
                        let current = nodes.get(&id).is_some_and(|now| Arc::ptr_eq(now, &pty));
                        if current {
                            nodes.remove(&id);
                        }
                        drop(nodes);
                        if current && let Some(links) = &host.links {
                            let _ = links.send((id, Linked::Unreached(cause)));
                        }
                        return;
                    }
                    Err((Unattached::Gone, on_exit)) => (on_exit, Exit::Gone),
                    Err((Unattached::Failed(_), on_exit)) => (on_exit, Exit::Lost),
                },
            };
            on_exit(exit);
        });
        Ok(())
    }

    fn connect<G>(
        &self,
        at: &Address,
        greet: impl FnOnce(&mut TcpStream) -> std::io::Result<G>,
    ) -> Result<(TcpStream, G), Unattached> {
        let silent = |e: std::io::Error| match at {
            Address::Here(_) => Unattached::Failed(e.to_string()),
            Address::Distro { .. } => Unattached::Unreached(e.to_string()),
        };
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
                .ok_or_else(|| Unattached::Unreached(format!("no distro host for {distro}")))?
                .reach(distro, *at)
                .map_err(|e| match e {
                    Unreached::Gone => Unattached::Gone,
                    Unreached::Failed(cause) => Unattached::Unreached(cause),
                })?,
        };
        stream
            .set_read_timeout(Some(GREET_TIMEOUT))
            .map_err(silent)?;
        let greeted = greet(&mut stream).map_err(silent)?;
        stream.set_read_timeout(None).map_err(silent)?;
        Ok((stream, greeted))
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

    fn kept_file(&self, id: K) -> PathBuf {
        self.kept.join(id.to_string())
    }
}

fn start_host(exe: &std::path::Path, spec: &host::Spec) -> Result<SocketAddr, String> {
    let mut host = std::process::Command::new(exe);
    host.arg("pty-host");
    let spec = serde_json::to_vec(spec).map_err(|e| e.to_string())?;
    match process_tree::announced(host, &spec) {
        Ok(address) if address.is_empty() => {
            Err("pty-host: exited before announcing its address".into())
        }
        Ok(address) => address
            .parse()
            .map_err(|e| format!("pty-host: {address}: {e}")),
        Err(e) => Err(format!("pty-host: {e}")),
    }
}

pub fn start_in_distro(start: distro::Start) -> Result<SocketAddr, String> {
    let (program, cmd, setup) = distro_commands(start);
    start_host(Path::new(&program), &host::Spec::of(&cmd, setup.as_ref()))
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
