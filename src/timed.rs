use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const EXIT_POLL: Duration = Duration::from_millis(20);

pub fn output(mut cmd: Command, limit: Duration) -> Option<Vec<u8>> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        let _ = tx.send(bytes);
    });
    let deadline = Instant::now() + limit;
    let bytes = rx.recv_timeout(limit).ok();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return bytes.filter(|_| status.success()),
            Ok(None) if bytes.is_some() && Instant::now() < deadline => {
                std::thread::sleep(EXIT_POLL)
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}
