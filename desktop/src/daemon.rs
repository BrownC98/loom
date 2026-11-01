use std::io::BufRead;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use crate::platform::hide_console;

const READY_PREFIX: &str = "loom: ";
const LOOM: &str = "loom";
const START_LIMIT: Duration = Duration::from_secs(30);

fn sibling() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe()
        .ok()?
        .with_file_name(format!("{LOOM}{}", std::env::consts::EXE_SUFFIX));
    exe.is_file().then_some(exe)
}

pub fn start() -> Result<String, String> {
    let mut command = match sibling() {
        Some(exe) => Command::new(exe),
        None => Command::new(LOOM),
    };
    command
        .arg("start")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    hide_console(&mut command);
    let mut child = command
        .spawn()
        .map_err(|e| format!("could not run `loom start`: {e}"))?;
    let stdout = child.stdout.take().ok_or("`loom start` has no stdout")?;
    let (sent, received) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let read = std::io::BufReader::new(stdout).read_line(&mut line);
        let _ = sent.send(read.map(|_| line));
    });
    let line = match received.recv_timeout(START_LIMIT) {
        Ok(read) => read.map_err(|e| e.to_string())?,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "`loom start` did not answer within {} s",
                START_LIMIT.as_secs()
            ));
        }
    };
    child.wait().map_err(|e| e.to_string())?;
    line.trim()
        .strip_prefix(READY_PREFIX)
        .map(str::to_owned)
        .ok_or_else(|| "the daemon did not start; see serve.log in the data folder".to_owned())
}
