use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const EXIT_POLL: Duration = Duration::from_millis(20);

pub fn output(mut cmd: Command, limit: Duration) -> Option<Vec<u8>> {
    #[cfg(unix)]
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(&mut cmd, || {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
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
    let Ok(bytes) = rx.recv_timeout(limit) else {
        stop(child);
        return None;
    };
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success().then_some(bytes),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(EXIT_POLL),
            _ => {
                stop(child);
                return None;
            }
        }
    }
}

fn stop(mut child: Child) {
    #[cfg(unix)]
    if let Ok(group) = i32::try_from(child.id()) {
        unsafe { libc::kill(-group, libc::SIGKILL) };
    }
    let _ = child.kill();
    let _ = child.wait();
}
