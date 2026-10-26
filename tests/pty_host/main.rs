#![cfg(target_os = "linux")]

use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const LOOM: &str = env!("CARGO_BIN_EXE_loom");
const LIMIT: Duration = Duration::from_secs(10);

const HELLO: u8 = 0;
const DATA: u8 = 1;
const KILL: u8 = 3;
const STREAMS: u8 = 7;
const ENDPOINT_FAILED: u8 = 8;
const HELD: u8 = 9;

fn word(text: impl AsRef<OsStr>) -> Value {
    json!({ "Unix": text.as_ref().as_bytes() })
}

fn host(home: &Path, spec: Value) -> (Child, String) {
    let mut child = Command::new(LOOM)
        .arg("pty-host")
        .current_dir(home)
        .env("HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(spec.to_string().as_bytes()).unwrap();
    drop(stdin);
    let mut address = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    (child, address.trim().to_owned())
}

fn env(home: &Path) -> Value {
    json!([["HOME", home.to_str().unwrap()], ["PATH", "/usr/bin:/bin"]])
}

fn write_frame(to: &mut impl Write, kind: u8, body: &[u8]) {
    let mut frame = vec![kind];
    frame.extend((body.len() as u32).to_le_bytes());
    frame.extend(body);
    to.write_all(&frame).unwrap();
}

fn read_frame(from: &mut impl Read) -> (u8, Vec<u8>) {
    let mut head = [0u8; 5];
    from.read_exact(&mut head).unwrap();
    let mut body = vec![0u8; u32::from_le_bytes(head[1..].try_into().unwrap()) as usize];
    from.read_exact(&mut body).unwrap();
    (head[0], body)
}

fn greeting(kind: u8, version: u32) -> Vec<u8> {
    let mut body = b"loom-pty".to_vec();
    body.extend(version.to_le_bytes());
    if kind == HELLO {
        body.extend(0u32.to_le_bytes());
    }
    body
}

fn attach(address: &str, kind: u8, version: u32) -> TcpStream {
    let mut link = TcpStream::connect(address).unwrap();
    link.set_read_timeout(Some(LIMIT)).unwrap();
    write_frame(&mut link, kind, &greeting(kind, version));
    assert_eq!(read_frame(&mut link).0, HELLO);
    link
}

fn frame_of(link: &mut TcpStream, kind: u8) -> Vec<u8> {
    loop {
        let (got, body) = read_frame(link);
        if got == kind {
            return body;
        }
    }
}

fn ended(child: &mut Child) -> bool {
    let until = Instant::now() + LIMIT;
    while Instant::now() < until {
        if child.try_wait().unwrap().is_some() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn open_fds(pid: u32) -> Vec<i32> {
    std::fs::read_dir(format!("/proc/{pid}/fd"))
        .unwrap()
        .map(|fd| fd.unwrap().file_name().to_str().unwrap().parse().unwrap())
        .collect()
}

fn file_limit(pid: u32, soft: u64) -> u64 {
    let pid = pid as libc::pid_t;
    let mut old = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    assert_eq!(
        unsafe { libc::prlimit(pid, libc::RLIMIT_NOFILE, std::ptr::null(), &mut old) },
        0
    );
    let new = libc::rlimit {
        rlim_cur: soft,
        rlim_max: old.rlim_max,
    };
    assert_eq!(
        unsafe { libc::prlimit(pid, libc::RLIMIT_NOFILE, &new, std::ptr::null_mut()) },
        0
    );
    old.rlim_cur
}

#[test]
fn an_endpoint_out_of_files_reports_it_and_serves_its_waiting_clients_later() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let at = home.join("run/node.sock");
    let spec = json!({
        "argv": [word("sh"), word("-c"), word("printf ready; read line")],
        "cwd": word(home),
        "env": env(home),
        "endpoint": word(&at),
    });
    let (mut child, address) = host(home, spec);
    let mut terminal = attach(&address, HELLO, 4);
    assert_eq!(frame_of(&mut terminal, DATA), b"ready");
    let highest = *open_fds(child.id()).iter().max().unwrap();
    let before = file_limit(child.id(), highest as u64 + 1);
    let mut clients = Vec::new();
    for _ in 0..8 {
        let mut client = UnixStream::connect(&at).unwrap();
        client.write_all(b"POST /held").unwrap();
        clients.push(client);
    }
    let failed = String::from_utf8(frame_of(&mut terminal, ENDPOINT_FAILED)).unwrap();
    assert!(failed.starts_with("accept: "), "{failed}");
    file_limit(child.id(), before);

    let mut streams = attach(&address, STREAMS, 2);
    assert_eq!(read_frame(&mut streams).0, HELD);
    let mut carried: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    while carried.windows(10).filter(|w| *w == b"POST /held").count() < clients.len() {
        let n = streams.read(&mut buf).unwrap();
        assert!(n > 0, "the stream link ended");
        carried.extend(&buf[..n]);
    }
    write_frame(&mut terminal, KILL, &[]);
    assert!(ended(&mut child));
}
