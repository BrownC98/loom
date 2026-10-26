use std::io;

use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncReadExt};

pub const MAX_CHUNK: usize = 64 * 1024;
const HEAD: usize = 9;

const OPEN: u8 = 1;
const DATA: u8 = 2;
const END: u8 = 3;
const CLOSE: u8 = 4;

#[derive(Debug, PartialEq)]
pub enum Frame {
    Open(u32),
    Data(u32, Bytes),
    End(u32),
    Close(u32),
}

pub fn encode(frame: &Frame, out: &mut Vec<u8>) {
    let (kind, id, body): (u8, u32, &[u8]) = match frame {
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
