mod wire;

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

pub const ENDPOINT_ENV: &str = "LOOM_SOCKET";
const BATCH_BYTES: usize = 256 * 1024;

struct Shared {
    out: mpsc::UnboundedSender<Frame>,
    streams: Mutex<Option<HashMap<u32, Inbound>>>,
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
    fn close(&self) {
        self.gone.store(true, Ordering::SeqCst);
    }

    fn reset(&self) {
        self.reset.store(true, Ordering::SeqCst);
        self.close();
    }
}

impl Shared {
    fn register(self: &Arc<Self>, id: u32) -> Stream {
        let (data, rx) = mpsc::unbounded_channel();
        let peer = Arc::new(Peer::default());
        let inbound = Inbound {
            data: Some(data),
            peer: peer.clone(),
        };
        match self.streams.lock().unwrap().as_mut() {
            Some(streams) => drop(streams.insert(id, inbound)),
            None => peer.reset(),
        }
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
        for (_, inbound) in self.streams.lock().unwrap().take().into_iter().flatten() {
            inbound.peer.reset();
        }
    }

    fn open_streams(&self) -> std::sync::MutexGuard<'_, Option<HashMap<u32, Inbound>>> {
        self.streams.lock().unwrap()
    }
}

pub struct Mux {
    shared: Arc<Shared>,
    next: std::sync::atomic::AtomicU32,
}

pub fn start<R, W>(reader: R, writer: W) -> (Mux, mpsc::UnboundedReceiver<Stream>)
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

pub fn accept<R, W>(reader: R, writer: W) -> mpsc::UnboundedReceiver<Stream>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    spawn(reader, writer).1
}

fn spawn<R, W>(reader: R, writer: W) -> (Arc<Shared>, mpsc::UnboundedReceiver<Stream>)
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (out, frames) = mpsc::unbounded_channel();
    let shared = Arc::new(Shared {
        out,
        streams: Mutex::new(Some(HashMap::new())),
    });
    let (events, events_rx) = mpsc::unbounded_channel();
    tokio::spawn(write_frames(writer, frames));
    tokio::spawn(read_frames(reader, shared.clone(), events));
    (shared, events_rx)
}

impl Mux {
    pub fn is_closed(&self) -> bool {
        self.shared.open_streams().is_none()
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
    events: mpsc::UnboundedSender<Stream>,
) {
    let mut reader = tokio::io::BufReader::new(reader);
    while let Ok(Some(frame)) = wire::read(&mut reader).await {
        match frame {
            Frame::Open(id) => {
                let _ = events.send(shared.register(id));
            }
            Frame::Data(id, bytes) => {
                if let Some(data) = shared
                    .open_streams()
                    .as_ref()
                    .and_then(|streams| streams.get(&id))
                    .and_then(|i| i.data.as_ref())
                {
                    let _ = data.send(bytes);
                }
            }
            Frame::End(id) => {
                if let Some(inbound) = shared
                    .open_streams()
                    .as_mut()
                    .and_then(|streams| streams.get_mut(&id))
                {
                    inbound.data = None;
                }
            }
            Frame::Close(id) => match shared
                .open_streams()
                .as_mut()
                .and_then(|streams| streams.remove(&id))
            {
                Some(Inbound {
                    data: Some(_),
                    peer,
                }) => peer.reset(),
                Some(Inbound { data: None, peer }) => peer.close(),
                None => {}
            },
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
        if let Some(streams) = self.shared.open_streams().as_mut() {
            streams.remove(&self.id);
        }
        if !self.is_gone() {
            let _ = self.shared.out.send(Frame::Close(self.id));
        }
    }
}

pub struct Door<A>(mpsc::UnboundedReceiver<(Stream, A)>);

impl<A> Door<A> {
    pub fn new() -> (mpsc::UnboundedSender<(Stream, A)>, Door<A>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (tx, Door(rx))
    }
}

impl<A: Clone + Send + Sync + 'static> axum::serve::Listener for Door<A> {
    type Io = Stream;
    type Addr = A;

    async fn accept(&mut self) -> (Stream, A) {
        match self.0.recv().await {
            Some(opened) => opened,
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> io::Result<A> {
        Err(io::ErrorKind::Unsupported.into())
    }
}

pub async fn splice<A, B>(near: A, far: B)
where
    A: AsyncRead + AsyncWrite,
    B: AsyncRead + AsyncWrite,
{
    let (mut from_near, mut to_near) = tokio::io::split(near);
    let (mut from_far, mut to_far) = tokio::io::split(far);
    let request = async {
        let _ = tokio::io::copy(&mut from_near, &mut to_far).await;
        let _ = to_far.shutdown().await;
        std::future::pending::<()>().await
    };
    let answer = async {
        let _ = tokio::io::copy(&mut from_far, &mut to_near).await;
    };
    tokio::select! {
        () = request => {}
        () = answer => {}
    }
}

pub fn carry<A: Send + 'static>(
    link: std::net::TcpStream,
    mut at: impl FnMut() -> A + Send + 'static,
    doors: mpsc::UnboundedSender<(Stream, A)>,
) -> io::Result<()> {
    link.set_nonblocking(true)?;
    let (reader, writer) = tokio::net::TcpStream::from_std(link)?.into_split();
    let mut events = accept(reader, writer);
    tokio::spawn(async move {
        while let Some(stream) = events.recv().await {
            if doors.send((stream, at())).is_err() {
                break;
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests;
