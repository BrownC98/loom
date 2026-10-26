use std::io::{self, Read, Write};

use crate::protocol::{PTY_HOST, STREAMS as STREAMS_LINK};

const MAGIC: &[u8; 8] = b"loom-pty";
pub const VERSION: u32 = PTY_HOST.now;
pub const STREAMS_SINCE: u32 = 4;
pub const ENDPOINT_FAILED_SINCE: u32 = 4;
pub const HELD_SINCE: u32 = 5;
const MAX_BODY: usize = 1 << 20;

const HELLO: u8 = 0;
const DATA: u8 = 1;
const RESIZE: u8 = 2;
const KILL: u8 = 3;
const EXIT: u8 = 4;
const PID: u8 = 5;
const SETUP_FAILED: u8 = 6;
const STREAMS: u8 = 7;
const ENDPOINT_FAILED: u8 = 8;
const HELD: u8 = 9;

#[derive(Debug, PartialEq)]
pub enum Frame {
    Hello { version: u32, pid: Option<u32> },
    Data(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Kill,
    Exit(u32),
    Pid(u32),
    SetupFailed(u32),
    Streams { version: u32 },
    EndpointFailed(String),
    Held(u32),
}

pub enum Link {
    Terminal { version: u32 },
    Streams,
}

pub fn write(to: &mut impl Write, frame: &Frame) -> io::Result<()> {
    let (kind, body) = match frame {
        Frame::Hello { version, pid } => {
            let mut body = MAGIC.to_vec();
            body.extend(version.to_le_bytes());
            body.extend(pid.unwrap_or(0).to_le_bytes());
            (HELLO, body)
        }
        Frame::Data(bytes) => (DATA, bytes.clone()),
        Frame::Resize { cols, rows } => (RESIZE, [cols.to_le_bytes(), rows.to_le_bytes()].concat()),
        Frame::Kill => (KILL, Vec::new()),
        Frame::Exit(code) => (EXIT, code.to_le_bytes().to_vec()),
        Frame::Pid(pid) => (PID, pid.to_le_bytes().to_vec()),
        Frame::SetupFailed(code) => (SETUP_FAILED, code.to_le_bytes().to_vec()),
        Frame::Streams { version } => (STREAMS, [&MAGIC[..], &version.to_le_bytes()].concat()),
        Frame::EndpointFailed(error) => (ENDPOINT_FAILED, error.as_bytes().to_vec()),
        Frame::Held(count) => (HELD, count.to_le_bytes().to_vec()),
    };
    let mut buf = Vec::with_capacity(5 + body.len());
    buf.push(kind);
    buf.extend((body.len() as u32).to_le_bytes());
    buf.extend(body);
    to.write_all(&buf)
}

pub fn read(from: &mut impl Read) -> io::Result<Frame> {
    let mut head = [0u8; 5];
    from.read_exact(&mut head)?;
    let len = u32::from_le_bytes(head[1..].try_into().expect("4바이트")) as usize;
    if len > MAX_BODY {
        return Err(invalid("frame too long"));
    }
    let mut body = vec![0u8; len];
    from.read_exact(&mut body)?;
    let u32_at = |at: usize| -> io::Result<u32> {
        let bytes = body.get(at..at + 4).ok_or_else(|| invalid("short frame"))?;
        Ok(u32::from_le_bytes(bytes.try_into().expect("4바이트")))
    };
    match head[0] {
        HELLO if body.starts_with(MAGIC) => Ok(Frame::Hello {
            version: u32_at(8)?,
            pid: Some(u32_at(12)?).filter(|&p| p != 0),
        }),
        HELLO => Err(invalid("not a loom pty-host")),
        STREAMS if body.starts_with(MAGIC) => Ok(Frame::Streams {
            version: u32_at(8)?,
        }),
        DATA => Ok(Frame::Data(body)),
        RESIZE if len == 4 => Ok(Frame::Resize {
            cols: u16::from_le_bytes([body[0], body[1]]),
            rows: u16::from_le_bytes([body[2], body[3]]),
        }),
        KILL => Ok(Frame::Kill),
        EXIT => Ok(Frame::Exit(u32_at(0)?)),
        PID => Ok(Frame::Pid(u32_at(0)?)),
        SETUP_FAILED => Ok(Frame::SetupFailed(u32_at(0)?)),
        ENDPOINT_FAILED => Ok(Frame::EndpointFailed(
            String::from_utf8_lossy(&body).into_owned(),
        )),
        HELD => Ok(Frame::Held(u32_at(0)?)),
        _ => Err(invalid("unknown frame")),
    }
}

pub struct Greeted {
    pub version: u32,
    pub pid: Option<u32>,
}

pub fn greet(stream: &mut (impl Read + Write)) -> io::Result<Greeted> {
    greet_as(
        stream,
        &Frame::Hello {
            version: VERSION,
            pid: None,
        },
    )
}

pub fn greet_streams(stream: &mut (impl Read + Write), host: u32) -> io::Result<Option<u32>> {
    let counted = host >= HELD_SINCE;
    let version = match counted {
        true => STREAMS_LINK.now,
        false => STREAMS_LINK.oldest,
    };
    greet_as(stream, &Frame::Streams { version })?;
    if !counted {
        return Ok(None);
    }
    match read(stream)? {
        Frame::Held(count) => Ok(Some(count)),
        _ => Err(invalid("pty-host did not count its held connections")),
    }
}

fn greet_as(stream: &mut (impl Read + Write), hello: &Frame) -> io::Result<Greeted> {
    write(stream, hello)?;
    match read(stream)? {
        Frame::Hello { version, pid } if PTY_HOST.accepts(version) => Ok(Greeted { version, pid }),
        Frame::Hello { version, .. } => Err(invalid(&format!("pty-host version {version}"))),
        _ => Err(invalid("pty-host did not greet")),
    }
}

pub fn welcome(stream: &mut (impl Read + Write), pid: Option<u32>) -> io::Result<Link> {
    let link = match read(stream)? {
        Frame::Hello { version, .. } => Link::Terminal { version },
        Frame::Streams { version } if version == STREAMS_LINK.now => Link::Streams,
        Frame::Streams { version } => {
            return Err(invalid(&format!("stream link version {version}")));
        }
        _ => return Err(invalid("client did not greet")),
    };
    write(
        stream,
        &Frame::Hello {
            version: VERSION,
            pid,
        },
    )?;
    Ok(link)
}

fn invalid(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.to_owned())
}
