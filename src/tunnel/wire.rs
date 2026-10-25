use std::io;

use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncReadExt};

pub const MAX_CHUNK: usize = 64 * 1024;
pub const VERSION: u32 = 1;
const HEAD: usize = 9;

const READY: u8 = 0;
const OPEN: u8 = 1;
const DATA: u8 = 2;
const END: u8 = 3;
const CLOSE: u8 = 4;

#[derive(Debug, PartialEq)]
pub enum Frame {
    Ready(u32),
    Open(u32),
    Data(u32, Bytes),
    End(u32),
    Close(u32),
}

pub fn encode(frame: &Frame, out: &mut Vec<u8>) {
    let (kind, id, body): (u8, u32, &[u8]) = match frame {
        Frame::Ready(version) => (READY, *version, &[]),
        Frame::Open(id) => (OPEN, *id, &[]),
        Frame::Data(id, bytes) => (DATA, *id, bytes),
        Frame::End(id) => (END, *id, &[]),
        Frame::Close(id) => (CLOSE, *id, &[]),
    };
    out.push(kind);
    out.extend(id.to_le_bytes());
    out.extend((body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
}

pub async fn read(from: &mut (impl AsyncRead + Unpin)) -> io::Result<Option<Frame>> {
    let mut head = [0u8; HEAD];
    if from.read(&mut head[..1]).await? == 0 {
        return Ok(None);
    }
    from.read_exact(&mut head[1..]).await?;
    let id = u32::from_le_bytes(head[1..5].try_into().expect("4바이트"));
    let len = u32::from_le_bytes(head[5..].try_into().expect("4바이트")) as usize;
    if len > MAX_CHUNK || (head[0] != DATA && len > 0) {
        return Err(invalid("frame too long"));
    }
    let mut body = vec![0u8; len];
    from.read_exact(&mut body).await?;
    Ok(Some(match head[0] {
        READY => Frame::Ready(id),
        OPEN => Frame::Open(id),
        DATA => Frame::Data(id, body.into()),
        END => Frame::End(id),
        CLOSE => Frame::Close(id),
        _ => return Err(invalid("unknown frame")),
    }))
}

fn invalid(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Trickle(Vec<u8>, usize);

    impl AsyncRead for Trickle {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            if let Some(&byte) = self.0.get(self.1) {
                buf.put_slice(&[byte]);
                self.1 += 1;
            }
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn frames_split_into_single_bytes_read_back_whole() {
        let frames = [
            Frame::Ready(VERSION),
            Frame::Open(7),
            Frame::Data(7, Bytes::from(vec![0xab; MAX_CHUNK])),
            Frame::Data(u32::MAX, Bytes::from_static(b"\0\r\n")),
            Frame::End(7),
            Frame::Close(7),
        ];
        let mut bytes = Vec::new();
        for frame in &frames {
            encode(frame, &mut bytes);
        }
        let mut from = Trickle(bytes, 0);
        for frame in frames {
            assert_eq!(read(&mut from).await.unwrap(), Some(frame));
        }
        assert_eq!(read(&mut from).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_cut_or_oversized_frame_is_an_error() {
        let mut bytes = Vec::new();
        encode(&Frame::Data(1, Bytes::from_static(b"abc")), &mut bytes);
        bytes.pop();
        assert!(read(&mut Trickle(bytes, 0)).await.is_err());
        let mut long = vec![DATA, 1, 0, 0, 0];
        long.extend((MAX_CHUNK as u32 + 1).to_le_bytes());
        assert!(read(&mut Trickle(long, 0)).await.is_err());
        let mut unknown = Vec::new();
        encode(&Frame::Close(1), &mut unknown);
        unknown[0] = 9;
        assert!(read(&mut Trickle(unknown, 0)).await.is_err());
    }
}
