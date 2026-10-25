use std::io::{self, Read, Write};

use crate::protocol::PTY_HOST;

const MAGIC: &[u8; 8] = b"loom-pty";
pub const VERSION: u32 = PTY_HOST.now;
pub const STREAMS_SINCE: u32 = 4;
pub const ENDPOINT_FAILED_SINCE: u32 = 4;
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

pub fn greet_streams(stream: &mut (impl Read + Write)) -> io::Result<()> {
    greet_as(
        stream,
        &Frame::Streams {
            version: crate::protocol::STREAMS,
        },
    )
    .map(drop)
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
        Frame::Streams { version } if version == crate::protocol::STREAMS => Link::Streams,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_1_bytes_are_frozen() {
        let mut out = Vec::new();
        write(
            &mut out,
            &Frame::Hello {
                version: 1,
                pid: Some(7),
            },
        )
        .unwrap();
        write(&mut out, &Frame::Resize { cols: 80, rows: 24 }).unwrap();
        write(&mut out, &Frame::Exit(3)).unwrap();
        assert_eq!(
            out,
            [
                &[0, 16, 0, 0, 0][..],
                b"loom-pty",
                &[1, 0, 0, 0, 7, 0, 0, 0],
                &[2, 4, 0, 0, 0, 80, 0, 24, 0],
                &[4, 4, 0, 0, 0, 3, 0, 0, 0],
            ]
            .concat()
        );
        let mut input = out.as_slice();
        assert_eq!(
            read(&mut input).unwrap(),
            Frame::Hello {
                version: 1,
                pid: Some(7)
            }
        );
        assert_eq!(
            read(&mut input).unwrap(),
            Frame::Resize { cols: 80, rows: 24 }
        );
        assert_eq!(read(&mut input).unwrap(), Frame::Exit(3));
    }

    #[test]
    fn version_2_pid_bytes_are_frozen() {
        let mut out = Vec::new();
        write(&mut out, &Frame::Pid(9)).unwrap();
        assert_eq!(out, [5, 4, 0, 0, 0, 9, 0, 0, 0]);
        assert_eq!(read(&mut out.as_slice()).unwrap(), Frame::Pid(9));
    }

    #[test]
    fn version_3_setup_failed_bytes_are_frozen() {
        let mut out = Vec::new();
        write(&mut out, &Frame::SetupFailed(2)).unwrap();
        assert_eq!(out, [6, 4, 0, 0, 0, 2, 0, 0, 0]);
        assert_eq!(read(&mut out.as_slice()).unwrap(), Frame::SetupFailed(2));
    }

    #[test]
    fn version_4_streams_bytes_are_frozen_and_pick_the_stream_link() {
        let mut out = Vec::new();
        write(&mut out, &Frame::Streams { version: 1 }).unwrap();
        assert_eq!(
            out,
            [&[7, 12, 0, 0, 0][..], b"loom-pty", &[1, 0, 0, 0]].concat()
        );
        let mut daemon = Duplex {
            input: std::io::Cursor::new(out),
            output: Vec::new(),
        };
        assert!(matches!(welcome(&mut daemon, Some(5)), Ok(Link::Streams)));
        assert_eq!(
            read(&mut daemon.output.as_slice()).unwrap(),
            Frame::Hello {
                version: VERSION,
                pid: Some(5)
            }
        );
        let mut newer = Vec::new();
        write(&mut newer, &Frame::Streams { version: 2 }).unwrap();
        let mut newer = Duplex {
            input: std::io::Cursor::new(newer),
            output: Vec::new(),
        };
        assert!(welcome(&mut newer, None).is_err());
        assert!(newer.output.is_empty());
    }

    #[test]
    fn version_4_endpoint_failed_bytes_are_frozen() {
        let mut out = Vec::new();
        write(&mut out, &Frame::EndpointFailed("no".into())).unwrap();
        assert_eq!(out, [8, 2, 0, 0, 0, b'n', b'o']);
        assert_eq!(
            read(&mut out.as_slice()).unwrap(),
            Frame::EndpointFailed("no".into())
        );
    }

    #[test]
    fn greet_accepts_known_versions_only() {
        let reply = |frame: Frame| {
            let mut bytes = Vec::new();
            write(&mut bytes, &frame).unwrap();
            Duplex {
                input: std::io::Cursor::new(bytes),
                output: Vec::new(),
            }
        };
        let mut old = reply(Frame::Hello {
            version: 1,
            pid: Some(42),
        });
        assert_eq!(greet(&mut old).unwrap().pid, Some(42));
        assert_eq!(
            read(&mut old.output.as_slice()).unwrap(),
            Frame::Hello {
                version: VERSION,
                pid: None
            }
        );
        let mut newer = reply(Frame::Hello {
            version: VERSION + 1,
            pid: Some(42),
        });
        assert_eq!(
            greet(&mut newer).err().unwrap().to_string(),
            format!("pty-host version {}", VERSION + 1)
        );
        let mut stranger = Duplex {
            input: std::io::Cursor::new(b"\0\x10\0\0\0HTTP/1.1 200 OK\n".to_vec()),
            output: Vec::new(),
        };
        assert_eq!(
            greet(&mut stranger).err().unwrap().to_string(),
            "not a loom pty-host"
        );
    }

    struct Duplex {
        input: std::io::Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Read for Duplex {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for Duplex {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
