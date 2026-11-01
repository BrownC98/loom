use std::io::{ErrorKind, Write};
use std::net::TcpStream;
use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

use tungstenite::Message;

use crate::wire::{self, Node, Notice};

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

pub fn nodes(base: &str) -> Result<Vec<Node>, String> {
    let body = ureq::get(format!("{base}/api/nodes"))
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    wire::nodes(&body)
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
    let stream = TcpStream::connect(host).map_err(|e| e.to_string())?;
    let (mut socket, _) = tungstenite::client(format!("ws://{host}/api/nodes/{id}/pty"), stream)
        .map_err(|e| e.to_string())?;
    socket
        .get_ref()
        .set_read_timeout(Some(POLL))
        .map_err(|e| e.to_string())?;
    socket
        .send(Message::text(wire::pace_front()))
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
                    socket.send(Message::text(wire::resize(cols, rows)))
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
            Ok(Message::Text(text)) => match wire::notice(&text) {
                Some(Notice::Size { cols, rows }) => Event::Size { cols, rows },
                Some(Notice::Settled) => Event::Settled,
                Some(Notice::Gone) => Event::Gone,
                Some(Notice::Beat) | None => continue,
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
