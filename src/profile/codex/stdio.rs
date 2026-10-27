use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

pub enum Failure {
    Run(std::io::Error),
    NoReply,
}

pub fn replies<const N: usize>(
    mut cmd: Command,
    calls: [(&str, Value); N],
    limit: Duration,
) -> Result<[Value; N], Failure> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(Failure::Run)?;
    let result = exchange(&mut child, calls, limit);
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn exchange<const N: usize>(
    child: &mut Child,
    calls: [(&str, Value); N],
    limit: Duration,
) -> Result<[Value; N], Failure> {
    let opening = [
        json!({ "method": "initialize", "id": 0, "params": { "clientInfo": { "name": "loom", "version": "0" } } }),
        json!({ "method": "initialized" }),
    ];
    let requests = calls
        .iter()
        .enumerate()
        .map(|(n, (method, params))| json!({ "method": method, "id": n + 1, "params": params }));
    let mut stdin = child.stdin.take().expect("stdin을 piped로 열었다");
    for line in opening.into_iter().chain(requests) {
        writeln!(stdin, "{line}").map_err(Failure::Run)?;
    }
    let stdout = child.stdout.take().expect("stdout을 piped로 열었다");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                return;
            }
        }
    });
    let deadline = Instant::now() + limit;
    let mut replies = [const { Value::Null }; N];
    let mut pending = N;
    while pending > 0 {
        let line = rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| Failure::NoReply)?;
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(slot) = msg["id"]
            .as_u64()
            .and_then(|id| usize::try_from(id).ok()?.checked_sub(1))
            .filter(|slot| *slot < N)
        else {
            continue;
        };
        replies[slot] = msg.get("result").cloned().unwrap_or(Value::Null);
        pending -= 1;
    }
    Ok(replies)
}
