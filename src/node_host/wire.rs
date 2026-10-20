//! 데몬과 PTY 호스트 사이의 프레임. 새 데몬은 옛 실행 파일이 띄운 호스트와도 말해야 한다(업그레이드).
//! 그래서 낸 판의 프레임은 바꾸지 않는다. 새 프레임이 필요하면 `VERSION`을 올리고, 옛 판 호스트에는 옛 프레임만 보낸다.
//! 프레임: 종류 1바이트, 본문 길이(u32 LE), 본문.

use std::io::{self, Read, Write};

/// 첫 프레임의 표지. 주소 파일의 포트를 다른 프로그램이 받았으면 여기서 가린다.
const MAGIC: &[u8; 8] = b"loom-pty";
/// 이 실행 파일이 말하는 판.
pub const VERSION: u32 = 1;
/// 본문 하나의 상한. 출력 조각(8KB)과 backlog(256KB)보다 크다. 넘으면 깨진 연결로 본다.
const MAX_BODY: usize = 1 << 20;

const HELLO: u8 = 0;
const DATA: u8 = 1;
const RESIZE: u8 = 2;
const KILL: u8 = 3;
const EXIT: u8 = 4;

#[derive(Debug, PartialEq)]
pub enum Frame {
    /// 연결의 첫 프레임. 데몬은 `pid` 없이, 호스트는 PTY에서 띄운 프로세스의 PID를 넣어 보낸다.
    Hello {
        version: u32,
        pid: Option<u32>,
    },
    /// 데몬→호스트: 키 입력. 호스트→데몬: 터미널 출력. 붙은 직후의 첫 출력은 backlog다.
    Data(Vec<u8>),
    Resize {
        cols: u16,
        rows: u16,
    },
    /// 프로세스를 끝낸다. 호스트는 그 뒤 `Exit`를 보낸다.
    Kill,
    /// 프로세스가 끝났다. 값은 종료 코드다.
    Exit(u32),
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
        _ => Err(invalid("unknown frame")),
    }
}

/// 데몬 쪽 인사. 호스트의 PID를 돌려준다. 이 실행 파일이 모르는 판의 호스트면 실패한다.
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

/// 호스트 쪽 인사. 데몬의 판과 관계없이 이 호스트의 판으로 답한다. 판을 맞추는 일은 데몬이 한다.
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

    /// 판 1의 바이트 모양. 바뀌면 옛 호스트와 말하지 못한다.
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

    /// 데몬은 자기 판 이하의 호스트와 말하고, 더 새 판과 다른 프로그램은 거절한다.
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
