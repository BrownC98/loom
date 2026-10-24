mod host;
mod wire;

use std::collections::{HashMap, VecDeque};
use std::fmt::Display;
use std::hash::Hash;
use std::io::{BufRead, Write};
use std::net::TcpStream;
use std::num::NonZeroU16;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use bytes::Bytes;
use portable_pty::CommandBuilder;
use tokio::sync::{broadcast, watch};

use crate::process_tree::{self, Snapshot};
use wire::Frame;

pub use host::run;

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

pub struct NodeHost<K> {
    dir: PathBuf,
    nodes: RwLock<HashMap<K, Arc<Pty>>>,
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

const KEPT_MODES: [u16; 14] = [
    47, 1047, 1049, 1, 9, 1000, 1001, 1002, 1003, 1004, 1005, 1006, 1015, 2004,
];

impl<K: Copy + Eq + Hash + Display + Send + Sync + 'static> NodeHost<K> {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            nodes: RwLock::default(),
        }
    }

    pub fn spawn(
        &self,
        id: K,
        cmd: CommandBuilder,
        setup: Option<CommandBuilder>,
        on_exit: impl FnOnce(Exit) + Send + 'static,
    ) -> Result<(), String> {
        let address = start_host(&cmd, setup.as_ref())?;
        std::fs::create_dir_all(&self.dir)
            .and_then(|()| std::fs::write(self.file(id), &address))
            .map_err(|e| e.to_string())?;
        self.resume(id, on_exit).map_err(|e| e.to_string())
    }

    pub fn resume(
        &self,
        id: K,
        on_exit: impl FnOnce(Exit) + Send + 'static,
    ) -> Result<(), Unattached> {
        let file = self.file(id);
        let connected = connect(&file);
        let (mut reader, pid) = match connected {
            Ok(found) => found,
            Err(e) => {
                let _ = std::fs::remove_file(&file);
                return Err(e);
            }
        };
        let host = reader
            .try_clone()
            .map_err(|e| Unattached::Silent(e.to_string()))?;
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
        self.nodes.write().unwrap().insert(id, pty.clone());
        std::thread::spawn(move || {
            let exit = loop {
                match wire::read(&mut reader) {
                    Ok(Frame::Data(chunk)) => {
                        pty.output.lock().unwrap().push(&chunk);
                    }
                    Ok(Frame::Exit(code)) => break Exit::Command(code),
                    Ok(Frame::SetupFailed(code)) => break Exit::Setup(code),
                    Ok(Frame::Pid(pid)) => *pty.pid.lock().unwrap() = Some(pid),
                    Ok(Frame::Resize { cols, rows }) => {
                        let mut sizing = pty.sizing.lock().unwrap();
                        if sizing.host == HostSize::Unreported {
                            pty.size.send_replace(PtySize { cols, rows });
                        }
                        sizing.host = HostSize::Reported;
                    }
                    Ok(_) => {}
                    Err(_) => break Exit::Lost,
                }
            };
            let _ = std::fs::remove_file(file);
            on_exit(exit);
        });
        Ok(())
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
            if let Ok((mut host, _)) = connect(&entry.path()) {
                let _ = wire::write(&mut host, &Frame::Kill);
            }
            let _ = std::fs::remove_file(entry.path());
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
}

fn start_host(cmd: &CommandBuilder, setup: Option<&CommandBuilder>) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe
        .to_str()
        .and_then(|path| path.strip_suffix(" (deleted)"))
        .map_or(exe.clone(), PathBuf::from);
    let mut host = std::process::Command::new(exe);
    host.arg("pty-host")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    process_tree::detach(&mut host);
    let mut child = host.spawn().map_err(|e| format!("pty-host: {e}"))?;
    let spec = serde_json::to_vec(&host::Spec::of(cmd, setup)).map_err(|e| e.to_string())?;
    let sent = child
        .stdin
        .take()
        .expect("stdin을 piped로 열었다")
        .write_all(&spec);
    let mut line = String::new();
    let stdout = child.stdout.take().expect("stdout을 piped로 열었다");
    let read = std::io::BufReader::new(stdout).read_line(&mut line);
    std::thread::spawn(move || child.wait());
    sent.and(read).map_err(|e| format!("pty-host: {e}"))?;
    match line.trim() {
        "" => Err("pty-host: exited before announcing its address".into()),
        address => Ok(address.to_owned()),
    }
}

fn connect(file: &std::path::Path) -> Result<(TcpStream, Option<u32>), Unattached> {
    let silent = |e: std::io::Error| Unattached::Silent(e.to_string());
    let address = std::fs::read_to_string(file).map_err(silent)?;
    let address = address
        .trim()
        .parse()
        .map_err(|e| Unattached::Silent(format!("{}: {e}", file.display())))?;
    let mut stream =
        TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).map_err(|e| match e.kind() {
            std::io::ErrorKind::ConnectionRefused => Unattached::Gone,
            _ => silent(e),
        })?;
    stream
        .set_read_timeout(Some(GREET_TIMEOUT))
        .map_err(silent)?;
    let pid = wire::greet(&mut stream).map_err(silent)?;
    stream.set_read_timeout(None).map_err(silent)?;
    Ok((stream, pid))
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
        let overflow = (self.bytes.len() + chunk.len()).saturating_sub(BACKLOG_BYTES);
        let dropped = overflow.min(self.bytes.len());
        if dropped > 0 {
            let seen = (dropped + MODE_SEQUENCE_MAX).min(self.bytes.len());
            let head: Vec<u8> = self.bytes.iter().take(seen).copied().collect();
            self.remember_modes(&head);
            self.bytes.drain(..dropped);
        }
        let keep = chunk.len().min(BACKLOG_BYTES);
        if keep < chunk.len() {
            self.remember_modes(&chunk[..chunk.len() - keep]);
        }
        self.bytes.extend(&chunk[chunk.len() - keep..]);
    }

    fn remember_modes(&mut self, bytes: &[u8]) {
        for (mode, on) in private_modes(bytes) {
            if !KEPT_MODES.contains(&mode) {
                continue;
            }
            match self.modes.iter_mut().find(|(m, _)| *m == mode) {
                Some(entry) => entry.1 = on,
                None => self.modes.push((mode, on)),
            }
        }
    }

    fn to_vec(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.bytes.len());
        for mode in KEPT_MODES {
            if let Some((_, on)) = self.modes.iter().find(|(m, _)| *m == mode) {
                out.extend(format!("\x1b[?{mode}{}", if *on { 'h' } else { 'l' }).bytes());
            }
        }
        out.extend(&self.bytes);
        out
    }
}

const MODE_SEQUENCE_MAX: usize = 32;

fn private_modes(bytes: &[u8]) -> Vec<(u16, bool)> {
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(start) = bytes[at..].windows(3).position(|w| w == b"\x1b[?") {
        let params = at + start + 3;
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
                found.push((mode, on));
            }
        }
    }
    found
}

#[derive(Default)]
pub struct Screen {
    pub text: String,
    pub to_cursor: String,
    pub cols: u16,
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

#[cfg(test)]
pub fn in_process(dir: &std::path::Path, id: impl Display, cmd: CommandBuilder) {
    let started = host::start(cmd, None).unwrap();
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(id.to_string()), started.address.to_string()).unwrap();
    std::thread::spawn(move || started.wait());
}

#[cfg(test)]
mod tests {
    use super::*;

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
            let started = host::start(sh("echo agent $$; exit 4"), Some(sh(setup))).unwrap();
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
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        std::fs::write(
            dir.path().join("1"),
            closed.local_addr().unwrap().to_string(),
        )
        .unwrap();
        drop(closed);
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

    #[test]
    fn backlog_keeps_terminal_modes_after_it_drops_their_bytes() {
        let mut backlog = Backlog::default();
        backlog.push(b"\x1b[?1049h\x1b[?7l\x1b[?1000h\x1b[?1002;1006h\x1b[?1003h\x1b[?1003l");
        backlog.push(&vec![b'a'; BACKLOG_BYTES]);
        let bytes = backlog.to_vec();
        let text = String::from_utf8_lossy(&bytes[..bytes.len() - BACKLOG_BYTES]).into_owned();
        assert_eq!(
            text,
            "\x1b[?1049h\x1b[?1000h\x1b[?1002h\x1b[?1003l\x1b[?1006h"
        );
        let mut parser = vt100::Parser::new(
            NonZeroU16::new(24).unwrap(),
            NonZeroU16::new(80).unwrap(),
            0,
        );
        parser.process(&bytes);
        assert!(parser.screen().alternate_screen());
        assert_eq!(
            parser.screen().mouse_protocol_mode(),
            vt100::MouseProtocolMode::ButtonMotion
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
