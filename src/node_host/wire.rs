use std::io::{self, Read, Write};

const MAGIC: &[u8; 8] = b"loom-pty";
pub const VERSION: u32 = 3;
const MAX_BODY: usize = 1 << 20;

const HELLO: u8 = 0;
const DATA: u8 = 1;
const RESIZE: u8 = 2;
const KILL: u8 = 3;
const EXIT: u8 = 4;
const PID: u8 = 5;
const SETUP_FAILED: u8 = 6;

#[derive(Debug, PartialEq)]
pub enum Frame {
    Hello { version: u32, pid: Option<u32> },
    Data(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Kill,
    Exit(u32),
    Pid(u32),
    SetupFailed(u32),
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
        DATA => Ok(Frame::Data(body)),
        RESIZE if len == 4 => Ok(Frame::Resize {
            cols: u16::from_le_bytes([body[0], body[1]]),
            rows: u16::from_le_bytes([body[2], body[3]]),
        }),
        KILL => Ok(Frame::Kill),
        EXIT => Ok(Frame::Exit(u32_at(0)?)),
        PID => Ok(Frame::Pid(u32_at(0)?)),
        SETUP_FAILED => Ok(Frame::SetupFailed(u32_at(0)?)),
        _ => Err(invalid("unknown frame")),
    }
}

pub fn greet(stream: &mut (impl Read + Write)) -> io::Result<Option<u32>> {
    write(
        stream,
        &Frame::Hello {
            version: VERSION,
            pid: None,
        },
    )?;
    match read(stream)? {
        Frame::Hello { version, pid } if (1..=VERSION).contains(&version) => Ok(pid),
        Frame::Hello { version, .. } => Err(invalid(&format!("pty-host version {version}"))),
        _ => Err(invalid("pty-host did not greet")),
    }
}

pub fn welcome(stream: &mut (impl Read + Write), pid: Option<u32>) -> io::Result<()> {
    match read(stream)? {
        Frame::Hello { .. } => write(
            stream,
            &Frame::Hello {
                version: VERSION,
                pid,
            },
        ),
        _ => Err(invalid("client did not greet")),
    }
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
        assert_eq!(greet(&mut old).unwrap(), Some(42));
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
            greet(&mut newer).unwrap_err().to_string(),
            format!("pty-host version {}", VERSION + 1)
        );
        let mut stranger = Duplex {
            input: std::io::Cursor::new(b"\0\x10\0\0\0HTTP/1.1 200 OK\n".to_vec()),
            output: Vec::new(),
        };
        assert_eq!(
            greet(&mut stranger).unwrap_err().to_string(),
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
