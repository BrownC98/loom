use std::collections::HashMap;
use std::io::{ErrorKind, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::{Duration, Instant};

use polling::{Event as Ready, Events, Poller};
use tungstenite::{Message, WebSocket};

use api::canvas::{CardSize, Layout};
use api::graph::pty::{Pace, PtyNotice, PtyRequest, Resize};
use api::graph::{GraphEvent, NodeView};
use api::repos::RepoView;
use api::settings::SettingsView;
use serde::Serialize;

const HTTP: &str = "http://";
const RECONNECT: Duration = Duration::from_secs(1);
const CONNECT_LIMIT: Duration = Duration::from_secs(5);
const HTTP_LIMIT: Duration = Duration::from_secs(10);
const LOST_AFTER: Duration = Duration::from_secs(10);
const FLUSH: Duration = Duration::from_millis(10);
const RESET: &[u8] = b"\x1bc";

pub enum Event {
    Size { cols: u16, rows: u16 },
    Settled,
    Backlog(Vec<u8>),
    Output(Vec<u8>),
    Gone,
    Closed(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LinkState {
    Connected,
    Reconnecting,
    Lost,
}

impl LinkState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Reconnecting => "reconnecting",
            Self::Lost => "lost",
        }
    }
}

enum Out {
    Input(Vec<u8>),
    Resize(u16, u16),
}

enum Cmd {
    Add(usize, Box<Conn>),
    Out(usize, Out),
    Close(usize),
    Pace(Pace),
    Opened(usize, u64, Box<Result<WebSocket<TcpStream>, String>>),
}

struct Hub {
    poller: Arc<Poller>,
    cmds: mpsc::Sender<Cmd>,
    keys: AtomicUsize,
}

impl Hub {
    fn send(&self, cmd: Cmd) {
        let _ = self.cmds.send(cmd);
        let _ = self.poller.notify();
    }
}

fn hub() -> &'static Hub {
    static HUB: OnceLock<Hub> = OnceLock::new();
    HUB.get_or_init(|| {
        let poller = Arc::new(Poller::new().expect("socket poller"));
        let (cmds, queue) = mpsc::channel();
        let worker = Worker {
            poller: poller.clone(),
            queue,
            cmds: cmds.clone(),
            conns: HashMap::new(),
            pace: Pace::Front,
        };
        std::thread::Builder::new()
            .name("link".to_owned())
            .spawn(move || worker.run())
            .expect("link thread");
        Hub {
            poller,
            cmds,
            keys: AtomicUsize::new(0),
        }
    })
}

pub fn pace(pace: Pace) {
    hub().send(Cmd::Pace(pace));
}

pub struct Pty {
    key: usize,
}

impl Pty {
    pub fn input(&self, bytes: Vec<u8>) {
        hub().send(Cmd::Out(self.key, Out::Input(bytes)));
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        hub().send(Cmd::Out(self.key, Out::Resize(cols, rows)));
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        hub().send(Cmd::Close(self.key));
    }
}

fn host(base: &str) -> Result<&str, String> {
    base.strip_prefix(HTTP)
        .map(|rest| rest.trim_end_matches('/'))
        .ok_or_else(|| format!("not a loopback http url: {base}"))
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_LIMIT))
        .build()
        .into()
}

pub fn put_settings(base: &str, patch: &serde_json::Value) -> Result<(), String> {
    agent()
        .put(format!("{base}/api/settings"))
        .header("content-type", "application/json")
        .send(patch.to_string())
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn get(base: &str, path: &str) -> Result<String, String> {
    agent()
        .get(format!("{base}{path}"))
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())
}

pub struct Snapshot {
    pub nodes: Vec<NodeView>,
    pub repos: Vec<RepoView>,
    pub layout: Layout,
    pub sizes: Vec<CardSize>,
}

pub struct Loaded {
    pub snapshot: Snapshot,
    pub settings: SettingsView,
}

fn read<T: serde::de::DeserializeOwned>(base: &str, path: &str) -> Result<T, String> {
    serde_json::from_str(&get(base, path)?).map_err(|e| format!("GET {path}: {e}"))
}

fn frame(request: &impl Serialize) -> Message {
    Message::text(serde_json::to_string(request).unwrap_or_default())
}

pub fn load(base: &str) -> Result<Loaded, String> {
    Ok(Loaded {
        snapshot: Snapshot {
            nodes: read(base, "/api/nodes")?,
            repos: read(base, "/api/repos")?,
            layout: read(base, "/api/spots")?,
            sizes: read(base, "/api/sizes")?,
        },
        settings: read(base, "/api/settings")?,
    })
}

pub enum Graph {
    Loaded(Box<Loaded>),
    Feed(Box<GraphEvent>),
    Failed(String),
    Link(LinkState),
}

pub fn watch(base: &str, graph: async_channel::Sender<Graph>) -> Result<(), String> {
    let host = host(base)?.to_owned();
    let (reload, wanted) = mpsc::channel::<()>();
    let base = base.to_owned();
    let sink = graph.clone();
    std::thread::Builder::new()
        .name("reload".to_owned())
        .spawn(move || {
            while wanted.recv().is_ok() {
                while wanted.try_recv().is_ok() {}
                let event = match load(&base) {
                    Ok(loaded) => Graph::Loaded(Box::new(loaded)),
                    Err(error) => Graph::Failed(error),
                };
                if sink.send_blocking(event).is_err() {
                    break;
                }
            }
        })
        .map_err(|e| e.to_string())?;
    add(Conn::new(host, Target::Events { graph, reload }));
    Ok(())
}

pub fn attach(base: &str, id: u32, events: async_channel::Sender<Event>) -> Result<Pty, String> {
    let host = host(base)?.to_owned();
    let key = add(Conn::new(
        host,
        Target::Pty {
            id,
            events,
            backlog: true,
        },
    ));
    Ok(Pty { key })
}

fn add(conn: Conn) -> usize {
    let hub = hub();
    let key = hub.keys.fetch_add(1, Ordering::Relaxed);
    hub.send(Cmd::Add(key, Box::new(conn)));
    key
}

fn socket(host: &str, path: &str) -> Result<WebSocket<TcpStream>, String> {
    let address = host
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or_else(|| format!("no address for {host}"))?;
    let stream = TcpStream::connect_timeout(&address, CONNECT_LIMIT).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(CONNECT_LIMIT))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(CONNECT_LIMIT))
        .map_err(|e| e.to_string())?;
    let (socket, _) =
        tungstenite::client(format!("ws://{host}{path}"), stream).map_err(|e| e.to_string())?;
    let stream = socket.get_ref();
    stream.set_read_timeout(None).map_err(|e| e.to_string())?;
    stream.set_write_timeout(None).map_err(|e| e.to_string())?;
    stream.set_nonblocking(true).map_err(|e| e.to_string())?;
    Ok(socket)
}

enum Target {
    Events {
        graph: async_channel::Sender<Graph>,
        reload: mpsc::Sender<()>,
    },
    Pty {
        id: u32,
        events: async_channel::Sender<Event>,
        backlog: bool,
    },
}

enum Fate {
    Down,
    Done,
    Dead,
}

struct Conn {
    target: Target,
    host: String,
    socket: Option<WebSocket<TcpStream>>,
    generation: u64,
    connecting: bool,
    retry: Instant,
    heard: Instant,
    limit: Duration,
    state: LinkState,
    down_since: Option<Instant>,
    opened_once: bool,
    done: bool,
    dirty: bool,
}

impl Conn {
    fn new(host: String, target: Target) -> Self {
        let now = Instant::now();
        Self {
            target,
            host,
            socket: None,
            generation: 0,
            connecting: false,
            retry: now,
            heard: now,
            limit: CONNECT_LIMIT,
            state: LinkState::Reconnecting,
            down_since: None,
            opened_once: false,
            done: false,
            dirty: false,
        }
    }

    fn path(&self) -> String {
        match &self.target {
            Target::Events { .. } => "/api/events".to_owned(),
            Target::Pty { id, .. } => format!("/api/nodes/{id}/pty"),
        }
    }

    fn report(&mut self, state: LinkState) -> bool {
        if self.state == state {
            return true;
        }
        self.state = state;
        match &self.target {
            Target::Events { graph, .. } => graph.send_blocking(Graph::Link(state)).is_ok(),
            Target::Pty { .. } => true,
        }
    }

    fn emit(&self, event: Event) -> Option<Fate> {
        match &self.target {
            Target::Pty { events, .. } if events.send_blocking(event).is_err() => Some(Fate::Dead),
            _ => None,
        }
    }

    fn push(&mut self, message: Message) -> Option<Fate> {
        let socket = self.socket.as_mut()?;
        match socket.send(message) {
            Ok(()) => None,
            Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {
                self.dirty = true;
                None
            }
            Err(_) => Some(Fate::Down),
        }
    }

    fn flush(&mut self) {
        if let Some(socket) = &mut self.socket {
            self.dirty = matches!(
                socket.flush(),
                Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock
            );
        }
    }

    fn out(&mut self, out: Out) -> Option<Fate> {
        let message = match out {
            Out::Input(bytes) => Message::binary(bytes),
            Out::Resize(cols, rows) => {
                if let Target::Pty { id, .. } = &self.target {
                    let _ = writeln!(
                        std::io::stderr(),
                        "loom-desktop: pty {id} resize {cols}x{rows}"
                    );
                }
                frame(&PtyRequest::Resize(Resize { cols, rows }))
            }
        };
        self.push(message)
    }

    fn pump(&mut self) -> Option<Fate> {
        loop {
            let socket = self.socket.as_mut()?;
            let message = match socket.read() {
                Ok(message) => message,
                Err(tungstenite::Error::Io(e))
                    if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
                {
                    self.flush();
                    return None;
                }
                Err(_) => return Some(Fate::Down),
            };
            self.heard = Instant::now();
            if let Some(fate) = self.handle(message) {
                return Some(fate);
            }
        }
    }

    fn handle(&mut self, message: Message) -> Option<Fate> {
        if matches!(message, Message::Close(_)) {
            return Some(Fate::Down);
        }
        match &mut self.target {
            Target::Events { reload, graph } => {
                let Message::Text(text) = message else {
                    return None;
                };
                match serde_json::from_str(&text) {
                    Ok(GraphEvent::Beat { quiet_ms }) => {
                        self.limit = Duration::from_millis(quiet_ms.into());
                        None
                    }
                    Ok(GraphEvent::Nodes | GraphEvent::ViewChanged) => {
                        reload.send(()).err().map(|_| Fate::Dead)
                    }
                    Ok(event) => graph
                        .send_blocking(Graph::Feed(Box::new(event)))
                        .err()
                        .map(|_| Fate::Dead),
                    Err(_) => None,
                }
            }
            Target::Pty { backlog, .. } => {
                let event = match message {
                    Message::Binary(bytes) => {
                        let bytes = bytes.to_vec();
                        if std::mem::take(backlog) {
                            Event::Backlog(bytes)
                        } else {
                            Event::Output(bytes)
                        }
                    }
                    Message::Text(text) => match serde_json::from_str(&text) {
                        Ok(PtyNotice::Size(Resize { cols, rows })) => Event::Size { cols, rows },
                        Ok(PtyNotice::Settled) => Event::Settled,
                        Ok(PtyNotice::Gone) => {
                            self.done = true;
                            let fate = self.emit(Event::Gone);
                            return fate.or(Some(Fate::Done));
                        }
                        Ok(PtyNotice::Beat { quiet_ms }) => {
                            self.limit = Duration::from_millis(quiet_ms.into());
                            return None;
                        }
                        Err(_) => return None,
                    },
                    _ => return None,
                };
                self.emit(event)
            }
        }
    }
}

struct Worker {
    poller: Arc<Poller>,
    queue: mpsc::Receiver<Cmd>,
    cmds: mpsc::Sender<Cmd>,
    conns: HashMap<usize, Conn>,
    pace: Pace,
}

impl Worker {
    fn run(mut self) {
        let mut ready = Events::new();
        loop {
            ready.clear();
            let timeout = self.timeout();
            let _ = self.poller.wait(&mut ready, timeout);
            let keys: Vec<usize> = ready.iter().map(|e| e.key).collect();
            while let Ok(cmd) = self.queue.try_recv() {
                self.command(cmd);
            }
            for key in keys {
                self.read(key);
            }
            self.tick();
        }
    }

    fn timeout(&self) -> Option<Duration> {
        let now = Instant::now();
        self.conns
            .values()
            .filter(|c| !c.done && !c.connecting)
            .map(|c| match c.socket {
                Some(_) if c.dirty => now + FLUSH,
                Some(_) => c.heard + c.limit,
                None => c.retry,
            })
            .min()
            .map(|at| at.saturating_duration_since(now))
    }

    fn command(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Add(key, conn) => {
                self.conns.insert(key, *conn);
                self.open(key);
            }
            Cmd::Out(key, out) => {
                let fate = self.conns.get_mut(&key).and_then(|c| c.out(out));
                self.settle(key, fate);
            }
            Cmd::Close(key) => self.forget(key),
            Cmd::Pace(pace) => {
                self.pace = pace;
                let keys: Vec<usize> = self.conns.keys().copied().collect();
                for key in keys {
                    let fate = self.conns.get_mut(&key).and_then(|c| c.push(frame(&pace)));
                    self.settle(key, fate);
                }
            }
            Cmd::Opened(key, generation, result) => self.opened(key, generation, *result),
        }
    }

    fn open(&mut self, key: usize) {
        let Some(conn) = self.conns.get_mut(&key) else {
            return;
        };
        conn.connecting = true;
        conn.generation += 1;
        let (generation, host, path) = (conn.generation, conn.host.clone(), conn.path());
        let (cmds, poller) = (self.cmds.clone(), self.poller.clone());
        std::thread::spawn(move || {
            let result = socket(&host, &path);
            let _ = cmds.send(Cmd::Opened(key, generation, Box::new(result)));
            let _ = poller.notify();
        });
    }

    fn opened(
        &mut self,
        key: usize,
        generation: u64,
        result: Result<WebSocket<TcpStream>, String>,
    ) {
        let Some(conn) = self.conns.get_mut(&key) else {
            return;
        };
        if conn.generation != generation {
            return;
        }
        conn.connecting = false;
        let now = Instant::now();
        let mut socket = match result {
            Ok(socket) => socket,
            Err(error) => {
                conn.retry = now + RECONNECT;
                let since = *conn.down_since.get_or_insert(now);
                let state = if now - since >= LOST_AFTER {
                    LinkState::Lost
                } else {
                    LinkState::Reconnecting
                };
                let first = conn.state != state && state == LinkState::Lost && !conn.opened_once;
                let mut alive = conn.report(state);
                if first {
                    alive &= conn.emit(Event::Closed(error)).is_none();
                }
                if !alive {
                    self.forget(key);
                }
                return;
            }
        };
        if unsafe { self.poller.add(socket.get_ref(), Ready::readable(key)) }.is_err() {
            conn.retry = now + RECONNECT;
            return;
        }
        let _ = socket.send(frame(&self.pace));
        conn.socket = Some(socket);
        conn.heard = now;
        conn.limit = CONNECT_LIMIT;
        conn.down_since = None;
        let mut fate = (!conn.report(LinkState::Connected)).then_some(Fate::Dead);
        if let Target::Pty { backlog, .. } = &mut conn.target {
            *backlog = true;
            if conn.opened_once {
                fate = fate.or(conn.emit(Event::Output(RESET.to_vec())));
            }
        }
        conn.opened_once = true;
        conn.flush();
        if fate.is_none() {
            fate = conn.pump();
        }
        self.settle(key, fate);
        self.rearm(key);
    }

    fn read(&mut self, key: usize) {
        let fate = self.conns.get_mut(&key).and_then(Conn::pump);
        self.settle(key, fate);
        self.rearm(key);
    }

    fn rearm(&self, key: usize) {
        if let Some(socket) = self.conns.get(&key).and_then(|c| c.socket.as_ref()) {
            let _ = self.poller.modify(socket.get_ref(), Ready::readable(key));
        }
    }

    fn settle(&mut self, key: usize, fate: Option<Fate>) {
        match fate {
            None => {}
            Some(Fate::Dead) => self.forget(key),
            Some(Fate::Done) => self.close(key),
            Some(Fate::Down) => self.down(key),
        }
    }

    fn close(&mut self, key: usize) {
        if let Some(socket) = self.conns.get_mut(&key).and_then(|c| c.socket.take()) {
            let _ = self.poller.delete(socket.get_ref());
        }
    }

    fn forget(&mut self, key: usize) {
        self.close(key);
        self.conns.remove(&key);
    }

    fn down(&mut self, key: usize) {
        self.close(key);
        let Some(conn) = self.conns.get_mut(&key) else {
            return;
        };
        let now = Instant::now();
        conn.retry = now + RECONNECT;
        conn.down_since.get_or_insert(now);
        conn.dirty = false;
        if !conn.report(LinkState::Reconnecting) {
            self.forget(key);
        }
    }

    fn tick(&mut self) {
        let now = Instant::now();
        let keys: Vec<usize> = self.conns.keys().copied().collect();
        for key in keys {
            let Some(conn) = self.conns.get_mut(&key) else {
                continue;
            };
            if conn.done || conn.connecting {
                continue;
            }
            if conn.socket.is_some() {
                if now >= conn.heard + conn.limit {
                    self.down(key);
                    if let Some(conn) = self.conns.get_mut(&key) {
                        conn.retry = now;
                    }
                } else if conn.dirty {
                    conn.flush();
                }
            } else if now >= conn.retry {
                self.open(key);
            }
        }
    }
}
