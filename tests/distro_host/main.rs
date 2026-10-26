#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const LOOM: &str = env!("CARGO_BIN_EXE_loom");
const LIMIT: Duration = Duration::from_secs(10);
const VERSION: u32 = 3;

const HELLO: u8 = 0;
const DATA: u8 = 1;
const EXIT: u8 = 4;
const STREAMS: u8 = 7;
const HELD: u8 = 9;

fn distro_host(home: &Path, count: &Path) -> (Child, String) {
    let launcher = format!(
        "echo launched >> '{}'; exec '{LOOM}' distro-host --inner",
        count.display()
    );
    let mut child = Command::new(LOOM)
        .arg("distro-host")
        .current_dir(home)
        .env("HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let spec = json!({ "argv": ["sh", "-c", launcher] });
    child
        .stdin
        .take()
        .unwrap()
        .write_all(spec.to_string().as_bytes())
        .unwrap();
    let mut address = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    let (address, pid) = address.trim().split_once(' ').unwrap();
    assert_eq!(pid.parse::<u32>().unwrap(), child.id());
    (child, address.to_owned())
}

fn frame(body: &[u8]) -> Vec<u8> {
    [&(body.len() as u32).to_le_bytes()[..], body].concat()
}

fn hello(version: u32) -> Vec<u8> {
    [&b"loom-distro"[..], &version.to_le_bytes()].concat()
}

fn read_body(from: &mut impl Read) -> Vec<u8> {
    let mut len = [0u8; 4];
    from.read_exact(&mut len).unwrap();
    let mut body = vec![0u8; u32::from_le_bytes(len) as usize];
    from.read_exact(&mut body).unwrap();
    body
}

fn ask(address: &str, question: Value) -> (TcpStream, Value) {
    let mut link = TcpStream::connect(address).unwrap();
    link.set_read_timeout(Some(LIMIT)).unwrap();
    link.write_all(&frame(&hello(VERSION))).unwrap();
    assert_eq!(read_body(&mut link), hello(VERSION));
    link.write_all(&frame(question.to_string().as_bytes()))
        .unwrap();
    let answer = serde_json::from_slice(&read_body(&mut link)).unwrap();
    (link, answer)
}

fn pty_frame(link: &mut TcpStream, kind: u8, body: &[u8]) {
    let mut frame = vec![kind];
    frame.extend((body.len() as u32).to_le_bytes());
    frame.extend(body);
    link.write_all(&frame).unwrap();
}

fn read_pty_frame(from: &mut impl Read) -> (u8, Vec<u8>) {
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

fn reach(address: &str, node: &str, kind: u8, version: u32) -> TcpStream {
    let (mut link, answer) = ask(address, json!({ "reach": node }));
    assert_eq!(answer, json!("reached"));
    pty_frame(&mut link, kind, &greeting(kind, version));
    assert_eq!(read_pty_frame(&mut link).0, HELLO);
    link
}

fn data_until(link: &mut TcpStream, text: &str) {
    let mut seen = String::new();
    while !seen.contains(text) {
        let (kind, body) = read_pty_frame(link);
        if kind == DATA {
            seen.push_str(&String::from_utf8_lossy(&body));
        }
    }
}

fn frame_of(link: &mut TcpStream, kind: u8) -> Vec<u8> {
    loop {
        let (got, body) = read_pty_frame(link);
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

fn launches(count: &Path) -> usize {
    std::fs::read_to_string(count).unwrap().lines().count()
}

fn inner_host_of(outer: &Child) -> u32 {
    let marker = format!("{LOOM}\0distro-host\0--inner\0");
    let parent = outer.id().to_string();
    std::fs::read_dir("/proc")
        .unwrap()
        .flatten()
        .filter(|entry| {
            std::fs::read(entry.path().join("cmdline")).is_ok_and(|c| c == marker.as_bytes())
        })
        .find(|entry| {
            std::fs::read_to_string(entry.path().join("stat")).is_ok_and(|stat| {
                stat.rsplit(')')
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .nth(1)
                    == Some(&parent)
            })
        })
        .and_then(|entry| entry.file_name().to_str()?.parse().ok())
        .expect("the outer host runs an inner host")
}

#[test]
fn a_distro_host_starts_and_carries_a_node_that_outlives_it() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let count = home.join("launches");
    let socket = home.join("run/node.sock");
    let (mut outer, address) = distro_host(home, &count);
    assert_eq!(launches(&count), 1);

    let (_, ran) = ask(
        &address,
        json!({ "run": { "argv": ["sh", "-c", "echo $WHO; pwd"], "cwd": home, "env": [["WHO", "distro"]] } }),
    );
    let stdout = format!("distro\n{}\n", home.display());
    assert_eq!(
        ran,
        json!({ "ran": { "code": 0, "stdout": stdout, "stderr": "" } })
    );

    let (_, started) = ask(
        &address,
        json!({ "start": {
            "program": LOOM,
            "command": {
                "argv": ["sh", "-c", "printf ready; read line; exit 3"],
                "cwd": home,
                "env": [["HOME", home], ["LOOM_SOCKET", socket]],
            },
            "setup": null,
        } }),
    );
    let node = started["started"].as_str().unwrap().to_owned();
    let mut terminal = reach(&address, &node, HELLO, 5);
    data_until(&mut terminal, "ready");
    let mut streams = reach(&address, &node, STREAMS, 2);
    assert_eq!(frame_of(&mut streams, HELD), 0u32.to_le_bytes());
    assert!(socket.exists());

    let inner = inner_host_of(&outer);
    outer.kill().unwrap();
    outer.wait().unwrap();
    let until = Instant::now() + LIMIT;
    while Path::new(&format!("/proc/{inner}")).exists() {
        assert!(Instant::now() < until, "the inner host ends with the outer");
        std::thread::sleep(Duration::from_millis(20));
    }
    TcpStream::connect(&node).expect("the node pty-host lives on");

    let (mut outer, address) = distro_host(home, &count);
    assert_eq!(launches(&count), 2);
    let mut terminal = reach(&address, &node, HELLO, 5);
    data_until(&mut terminal, "ready");
    pty_frame(&mut terminal, DATA, b"\n");
    assert_eq!(frame_of(&mut terminal, EXIT), 3u32.to_le_bytes());

    let until = Instant::now() + LIMIT;
    while ask(&address, json!({ "reach": node })).1 != json!("gone") {
        assert!(Instant::now() < until, "the ended pty-host is gone");
        std::thread::sleep(Duration::from_millis(20));
    }

    let mut other = TcpStream::connect(&address).unwrap();
    other.set_read_timeout(Some(LIMIT)).unwrap();
    other.write_all(&frame(&hello(VERSION + 1))).unwrap();
    assert!(
        ended(&mut outer),
        "a daemon of another version ends the distro host"
    );
}

#[test]
fn a_served_process_reports_its_pid_and_outlives_the_distro_host() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let count = home.join("launches");
    let (mut outer, address) = distro_host(home, &count);
    let (mut link, spawned) = ask(
        &address,
        json!({ "serve": { "argv": ["sleep", "30"], "cwd": home, "env": [] } }),
    );
    let pid = spawned["spawned"].as_u64().unwrap();
    outer.kill().unwrap();
    outer.wait().unwrap();
    let mut rest = Vec::new();
    let _ = link.read_to_end(&mut rest);
    assert!(Path::new(&format!("/proc/{pid}")).exists());
    Command::new("kill").arg(pid.to_string()).status().unwrap();
}
