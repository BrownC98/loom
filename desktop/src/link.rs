use std::io::{ErrorKind, Write};
use std::net::TcpStream;
use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

use tungstenite::Message;

use api::canvas::{CardSize, Layout};
use api::graph::pty::{Pace, PtyNotice, PtyRequest, Resize};
use api::graph::{GraphEvent, NodeView};
use api::repos::RepoView;
use serde::Serialize;

const POLL: Duration = Duration::from_millis(10);
const HTTP: &str = "http://";

pub enum Event {
    Size { cols: u16, rows: u16 },
    Settled,
    Backlog(Vec<u8>),
    Output(Vec<u8>),
    Gone,
    Closed(String),
}

enum Out {
    Input(Vec<u8>),
    Resize(u16, u16),
}

pub struct Pty {
    out: mpsc::Sender<Out>,
}

impl Pty {
    pub fn input(&self, bytes: Vec<u8>) {
        let _ = self.out.send(Out::Input(bytes));
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = self.out.send(Out::Resize(cols, rows));
    }
}

fn host(base: &str) -> Result<&str, String> {
    base.strip_prefix(HTTP)
        .map(|rest| rest.trim_end_matches('/'))
        .ok_or_else(|| format!("not a loopback http url: {base}"))
}

fn get(base: &str, path: &str) -> Result<String, String> {
    ureq::get(format!("{base}{path}"))
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

fn read<T: serde::de::DeserializeOwned>(base: &str, path: &str) -> Result<T, String> {
    serde_json::from_str(&get(base, path)?).map_err(|e| format!("GET {path}: {e}"))
}

fn frame(request: &impl Serialize) -> Result<Message, String> {
    serde_json::to_string(request)
        .map(Message::text)
        .map_err(|e| e.to_string())
}

pub fn load(base: &str) -> Result<Snapshot, String> {
    Ok(Snapshot {
        nodes: read(base, "/api/nodes")?,
        repos: read(base, "/api/repos")?,
        layout: read(base, "/api/spots")?,
        sizes: read(base, "/api/sizes")?,
    })
}

pub enum Graph {
    Loaded(Box<Snapshot>),
    Failed(String),
}

pub fn watch(base: &str, graph: async_channel::Sender<Graph>) -> Result<(), String> {
    let base = base.to_owned();
    let host = host(&base)?.to_owned();
    std::thread::Builder::new()
        .name("events".to_owned())
        .spawn(move || {
            let reason = follow(&base, &host, &graph).err().unwrap_or_default();
            let _ = graph.send_blocking(Graph::Failed(reason));
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn follow(base: &str, host: &str, graph: &async_channel::Sender<Graph>) -> Result<(), String> {
    let mut socket = socket(host, "/api/events")?;
    socket
        .send(frame(&Pace::Front)?)
        .map_err(|e| e.to_string())?;
    loop {
        match socket.read().map_err(|e| e.to_string())? {
            Message::Text(text)
                if matches!(
                    serde_json::from_str(&text),
                    Ok(GraphEvent::Nodes | GraphEvent::ViewChanged)
                ) =>
            {
                let event = match load(base) {
                    Ok(snapshot) => Graph::Loaded(Box::new(snapshot)),
                    Err(error) => Graph::Failed(error),
                };
                if graph.send_blocking(event).is_err() {
                    return Ok(());
                }
            }
            Message::Close(_) => return Ok(()),
            _ => {}
        }
    }
}

fn socket(host: &str, path: &str) -> Result<tungstenite::WebSocket<TcpStream>, String> {
    let stream = TcpStream::connect(host).map_err(|e| e.to_string())?;
    let (socket, _) =
        tungstenite::client(format!("ws://{host}{path}"), stream).map_err(|e| e.to_string())?;
    Ok(socket)
}

pub fn attach(base: &str, id: u32, events: async_channel::Sender<Event>) -> Result<Pty, String> {
    let host = host(base)?.to_owned();
    let (out, queue) = mpsc::channel();
    std::thread::Builder::new()
        .name(format!("pty-{id}"))
        .spawn(move || {
            let reason = run(&host, id, &events, &queue).err().unwrap_or_default();
            let _ = events.send_blocking(Event::Closed(reason));
        })
        .map_err(|e| e.to_string())?;
    Ok(Pty { out })
}

fn run(
    host: &str,
    id: u32,
    events: &async_channel::Sender<Event>,
    queue: &mpsc::Receiver<Out>,
) -> Result<(), String> {
    let mut socket = socket(host, &format!("/api/nodes/{id}/pty"))?;
    socket
        .get_ref()
        .set_read_timeout(Some(POLL))
        .map_err(|e| e.to_string())?;
    socket
        .send(frame(&Pace::Front)?)
        .map_err(|e| e.to_string())?;
    let mut backlog = true;
    loop {
        loop {
            let sent = match queue.try_recv() {
                Ok(Out::Input(bytes)) => socket.send(Message::binary(bytes)),
                Ok(Out::Resize(cols, rows)) => {
                    let _ = writeln!(
                        std::io::stderr(),
                        "loom-desktop: pty {id} resize {cols}x{rows}"
                    );
                    socket.send(frame(&PtyRequest::Resize(Resize { cols, rows }))?)
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            };
            sent.map_err(|e| e.to_string())?;
        }
        let event = match socket.read() {
            Ok(Message::Binary(bytes)) => {
                let bytes = bytes.to_vec();
                if std::mem::take(&mut backlog) {
                    Event::Backlog(bytes)
                } else {
                    Event::Output(bytes)
                }
            }
            Ok(Message::Text(text)) => match serde_json::from_str(&text) {
                Ok(PtyNotice::Size(Resize { cols, rows })) => Event::Size { cols, rows },
                Ok(PtyNotice::Settled) => Event::Settled,
                Ok(PtyNotice::Gone) => Event::Gone,
                Ok(PtyNotice::Beat { .. }) | Err(_) => continue,
            },
            Ok(Message::Close(_)) => return Ok(()),
            Ok(_) => continue,
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
            {
                continue;
            }
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                return Ok(());
            }
            Err(e) => return Err(e.to_string()),
        };
        if events.send_blocking(event).is_err() {
            return Ok(());
        }
    }
}
