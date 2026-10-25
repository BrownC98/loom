mod supervisor;
mod wire;

pub use supervisor::{Launch, Pace, Tunnels};

use std::collections::HashMap;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::sync::mpsc;

use wire::{Frame, MAX_CHUNK};

const BATCH_BYTES: usize = 256 * 1024;

struct Shared {
    out: mpsc::UnboundedSender<Frame>,
    streams: Mutex<HashMap<u32, Inbound>>,
}

struct Inbound {
    data: Option<mpsc::UnboundedSender<Bytes>>,
    peer: Arc<Peer>,
}

#[derive(Default)]
struct Peer {
    reset: AtomicBool,
    gone: AtomicBool,
}

impl Peer {
    fn close(&self, reset: bool) {
        self.reset.fetch_or(reset, Ordering::SeqCst);
        self.gone.store(true, Ordering::SeqCst);
    }
}

impl Shared {
    fn register(self: &Arc<Self>, id: u32) -> Stream {
        let (data, rx) = mpsc::unbounded_channel();
        let peer = Arc::new(Peer::default());
        self.streams.lock().unwrap().insert(
            id,
            Inbound {
                data: Some(data),
                peer: peer.clone(),
            },
        );
        Stream {
            id,
            data: rx,
            pending: Bytes::new(),
            peer,
            shared: self.clone(),
            ended: false,
        }
    }

    fn reset_all(&self) {
        for (_, inbound) in self.streams.lock().unwrap().drain() {
            inbound.peer.close(true);
        }
    }
}

pub enum Event {
    Ready,
    Opened(Stream),
}

#[cfg(any(unix, test))]
pub struct Mux {
    shared: Arc<Shared>,
    next: std::sync::atomic::AtomicU32,
}

#[cfg(any(unix, test))]
pub fn start<R, W>(reader: R, writer: W) -> (Mux, mpsc::UnboundedReceiver<Event>)
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (shared, events) = spawn(reader, writer);
    let mux = Mux {
        shared,
        next: std::sync::atomic::AtomicU32::new(1),
    };
    (mux, events)
}

pub fn accept<R, W>(reader: R, writer: W) -> mpsc::UnboundedReceiver<Event>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    spawn(reader, writer).1
}

fn spawn<R, W>(reader: R, writer: W) -> (Arc<Shared>, mpsc::UnboundedReceiver<Event>)
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (out, frames) = mpsc::unbounded_channel();
    let shared = Arc::new(Shared {
        out,
        streams: Mutex::default(),
    });
    let (events, events_rx) = mpsc::unbounded_channel();
    tokio::spawn(write_frames(writer, frames));
    tokio::spawn(read_frames(reader, shared.clone(), events));
    (shared, events_rx)
}

#[cfg(any(unix, test))]
impl Mux {
    #[cfg(unix)]
    pub fn ready(&self) {
        let _ = self.shared.out.send(Frame::Ready);
    }

    pub fn open(&self) -> Stream {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let stream = self.shared.register(id);
        let _ = self.shared.out.send(Frame::Open(id));
        stream
    }
}

async fn write_frames(
    mut writer: impl AsyncWrite + Unpin,
    mut frames: mpsc::UnboundedReceiver<Frame>,
) {
    let mut buf = Vec::new();
    while let Some(frame) = frames.recv().await {
        buf.clear();
        wire::encode(&frame, &mut buf);
        while buf.len() < BATCH_BYTES
            && let Ok(frame) = frames.try_recv()
        {
            wire::encode(&frame, &mut buf);
        }
        if writer.write_all(&buf).await.is_err() || writer.flush().await.is_err() {
            break;
        }
    }
}

async fn read_frames(
    reader: impl AsyncRead + Unpin,
    shared: Arc<Shared>,
    events: mpsc::UnboundedSender<Event>,
) {
    let mut reader = tokio::io::BufReader::new(reader);
    while let Ok(Some(frame)) = wire::read(&mut reader).await {
        match frame {
            Frame::Ready => {
                let _ = events.send(Event::Ready);
            }
            Frame::Open(id) => {
                let _ = events.send(Event::Opened(shared.register(id)));
            }
            Frame::Data(id, bytes) => {
                if let Some(data) = shared
                    .streams
                    .lock()
                    .unwrap()
                    .get(&id)
                    .and_then(|i| i.data.as_ref())
                {
                    let _ = data.send(bytes);
                }
            }
            Frame::End(id) => {
                if let Some(inbound) = shared.streams.lock().unwrap().get_mut(&id) {
                    inbound.data = None;
                }
            }
            Frame::Close(id) => {
                if let Some(inbound) = shared.streams.lock().unwrap().remove(&id) {
                    inbound.peer.close(inbound.data.is_some());
                }
            }
        }
    }
    shared.reset_all();
}

pub struct Stream {
    id: u32,
    data: mpsc::UnboundedReceiver<Bytes>,
    pending: Bytes,
    peer: Arc<Peer>,
    shared: Arc<Shared>,
    ended: bool,
}

impl Stream {
    fn is_reset(&self) -> bool {
        self.peer.reset.load(Ordering::SeqCst)
    }

    fn is_gone(&self) -> bool {
        self.peer.gone.load(Ordering::SeqCst)
    }
}

impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        while this.pending.is_empty() {
            match this.data.poll_recv(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(bytes)) => this.pending = bytes,
                Poll::Ready(None) if this.is_reset() => {
                    return Poll::Ready(Err(io::ErrorKind::ConnectionReset.into()));
                }
                Poll::Ready(None) => return Poll::Ready(Ok(())),
            }
        }
        let n = buf.remaining().min(this.pending.len());
        buf.put_slice(&this.pending.split_to(n));
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.ended || self.is_gone() {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        let n = buf.len().min(MAX_CHUNK);
        if n == 0 {
            return Poll::Ready(Ok(0));
        }
        let frame = Frame::Data(self.id, Bytes::copy_from_slice(&buf[..n]));
        Poll::Ready(match self.shared.out.send(frame) {
            Ok(()) => Ok(n),
            Err(_) => Err(io::ErrorKind::BrokenPipe.into()),
        })
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if !this.ended && !this.is_gone() {
            this.ended = true;
            let _ = this.shared.out.send(Frame::End(this.id));
        }
        Poll::Ready(Ok(()))
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.shared.streams.lock().unwrap().remove(&self.id);
        if !self.is_gone() {
            let _ = self.shared.out.send(Frame::Close(self.id));
        }
    }
}

pub struct Door(mpsc::UnboundedReceiver<Stream>);

impl Door {
    pub fn new() -> (mpsc::UnboundedSender<Stream>, Door) {
        let (tx, rx) = mpsc::unbounded_channel();
        (tx, Door(rx))
    }
}

impl axum::serve::Listener for Door {
    type Io = Stream;
    type Addr = ();

    async fn accept(&mut self) -> (Stream, ()) {
        match self.0.recv().await {
            Some(stream) => (stream, ()),
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(unix)]
pub async fn relay(
    sock: &std::path::Path,
    from: impl AsyncRead + Unpin + Send + 'static,
    to: impl AsyncWrite + Unpin + Send + 'static,
) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let listener = bind(sock)?;
    let bound = std::fs::metadata(sock)?.ino();
    let (mux, mut events) = start(from, to);
    mux.ready();
    let mut conns = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((mut client, _)) => {
                    let mut stream = mux.open();
                    conns.spawn(async move {
                        let _ = tokio::io::copy_bidirectional(&mut client, &mut stream).await;
                    });
                }
                Err(e) => {
                    eprintln!("loom: tunnel accept: {e}");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            },
            event = events.recv() => if event.is_none() {
                break;
            },
            Some(_) = conns.join_next(), if !conns.is_empty() => {}
        }
    }
    if std::fs::metadata(sock).is_ok_and(|m| m.ino() == bound) {
        let _ = std::fs::remove_file(sock);
    }
    Ok(())
}

#[cfg(unix)]
fn bind(sock: &std::path::Path) -> io::Result<tokio::net::UnixListener> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let dir = sock
        .parent()
        .ok_or_else(|| io::Error::other("the socket path has no folder"))?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    match std::fs::remove_file(sock) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    tokio::net::UnixListener::bind(sock)
}

#[cfg(test)]
mod tests;
