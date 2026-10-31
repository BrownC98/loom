use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[cfg(unix)]
pub fn own_group(cmd: &mut Command) {
    std::os::unix::process::CommandExt::process_group(cmd, 0);
}

#[cfg(windows)]
pub fn own_group(_: &mut Command) {}

#[cfg(unix)]
pub fn own_session(cmd: &mut Command) {
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(cmd, || {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(windows)]
pub fn own_session(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
}

#[cfg(unix)]
pub fn daemon_child(cmd: &mut Command) {
    own_session(cmd);
}

#[cfg(windows)]
pub fn daemon_child(cmd: &mut Command) {
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        unsafe { SetHandleInformation(GetStdHandle(which), HANDLE_FLAG_INHERIT, 0) };
    }
    own_session(cmd);
}

pub fn output(cmd: Command, limit: Duration) -> Option<Vec<u8>> {
    let out = finished(cmd, limit).ok()?;
    out.status.success().then_some(out.stdout)
}

pub struct Output {
    pub status: std::process::ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub fn finished(mut cmd: Command, limit: Duration) -> Result<Output, String> {
    own_session(&mut cmd);
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let stdout = drained(child.stdout.take());
    let stderr = drained(child.stderr.take());
    let deadline = Instant::now() + limit;
    let not_ended = || format!("did not end in {} s", limit.as_secs());
    let (Ok(stdout), Ok(stderr)) = (
        stdout.recv_timeout(limit),
        stderr.recv_timeout(deadline.saturating_duration_since(Instant::now())),
    ) else {
        stop(child);
        return Err(not_ended());
    };
    let status = wait_until(child, deadline).ok_or_else(not_ended)?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

pub async fn ended_within(
    mut cmd: tokio::process::Command,
    limit: Duration,
) -> Result<Output, String> {
    own_session(cmd.as_std_mut());
    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| e.to_string())?;
    #[cfg(unix)]
    let group = child.id().and_then(|pid| i32::try_from(pid).ok());
    match tokio::time::timeout(limit, child.wait_with_output()).await {
        Ok(out) => out
            .map(|out| Output {
                status: out.status,
                stdout: out.stdout,
                stderr: out.stderr,
            })
            .map_err(|e| e.to_string()),
        Err(_) => {
            #[cfg(unix)]
            if let Some(group) = group {
                unsafe { libc::kill(-group, libc::SIGKILL) };
            }
            Err(format!(
                "did not end in {} s, so loom ended it",
                limit.as_secs()
            ))
        }
    }
}

fn drained(from: Option<impl Read + Send + 'static>) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut from) = from {
            let _ = from.read_to_end(&mut bytes);
        }
        let _ = tx.send(bytes);
    });
    rx
}

pub fn wait_until(mut child: Child, deadline: Instant) -> Option<std::process::ExitStatus> {
    let ended = match Exit::watch(child.id()) {
        Ok(Some(exit)) => exit.wait(deadline).is_ok(),
        Ok(None) => true,
        Err(_) => false,
    };
    if !ended {
        stop(child);
        return None;
    }
    child.wait().ok()
}

pub fn stop(mut child: Child) {
    #[cfg(unix)]
    if let Ok(group) = i32::try_from(child.id()) {
        unsafe { libc::kill(-group, libc::SIGKILL) };
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn unwatched(pid: u32, gone: i32) -> Result<Option<Exit>, String> {
    let e = std::io::Error::last_os_error();
    match e.raw_os_error() == Some(gone) {
        true => Ok(None),
        false => Err(format!("cannot watch process {pid} for its end: {e}")),
    }
}

fn not_ended(pid: u32) -> String {
    format!("process {pid} did not end in time")
}

#[cfg(target_os = "linux")]
pub struct Exit {
    pid: u32,
    fd: std::os::fd::OwnedFd,
}

#[cfg(target_os = "linux")]
impl Exit {
    pub fn watch(pid: u32) -> Result<Option<Self>, String> {
        use std::os::fd::FromRawFd;
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) };
        match libc::c_int::try_from(fd) {
            Ok(fd) if fd >= 0 => Ok(Some(Exit {
                pid,
                fd: unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) },
            })),
            _ => unwatched(pid, libc::ESRCH),
        }
    }

    pub fn wait(self, deadline: Instant) -> Result<(), String> {
        use std::os::fd::AsRawFd;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let mut ready = libc::pollfd {
                fd: self.fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let ms = libc::c_int::try_from(left.as_millis()).unwrap_or(libc::c_int::MAX);
            match unsafe { libc::poll(&mut ready, 1, ms) } {
                0 => return Err(not_ended(self.pid)),
                1.. => return Ok(()),
                _ => {
                    let e = std::io::Error::last_os_error();
                    if e.kind() != std::io::ErrorKind::Interrupted {
                        return Err(e.to_string());
                    }
                }
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub struct Exit {
    pid: u32,
    queue: std::os::fd::OwnedFd,
}

#[cfg(target_os = "macos")]
impl Exit {
    pub fn watch(pid: u32) -> Result<Option<Self>, String> {
        use std::os::fd::FromRawFd;
        let queue = unsafe { libc::kqueue() };
        if queue < 0 {
            return Err(format!(
                "cannot watch process {pid} for its end: {}",
                std::io::Error::last_os_error()
            ));
        }
        let queue = unsafe { std::os::fd::OwnedFd::from_raw_fd(queue) };
        let change = libc::kevent {
            ident: pid as libc::uintptr_t,
            filter: libc::EVFILT_PROC,
            flags: libc::EV_ADD | libc::EV_ONESHOT,
            fflags: libc::NOTE_EXIT,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        let added = unsafe {
            use std::os::fd::AsRawFd;
            libc::kevent(
                queue.as_raw_fd(),
                &change,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        match added {
            0 => Ok(Some(Exit { pid, queue })),
            _ => unwatched(pid, libc::ESRCH),
        }
    }

    pub fn wait(self, deadline: Instant) -> Result<(), String> {
        use std::os::fd::AsRawFd;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let limit = libc::timespec {
                tv_sec: left.as_secs() as libc::time_t,
                tv_nsec: left.subsec_nanos() as libc::c_long,
            };
            let mut event: libc::kevent = unsafe { std::mem::zeroed() };
            match unsafe {
                libc::kevent(
                    self.queue.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    &mut event,
                    1,
                    &limit,
                )
            } {
                0 => return Err(not_ended(self.pid)),
                1.. => return Ok(()),
                _ => {
                    let e = std::io::Error::last_os_error();
                    if e.kind() != std::io::ErrorKind::Interrupted {
                        return Err(e.to_string());
                    }
                }
            }
        }
    }
}

#[cfg(windows)]
pub struct Exit {
    pid: u32,
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl Exit {
    pub fn watch(pid: u32) -> Result<Option<Self>, String> {
        use windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER;
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        match handle.is_null() {
            false => Ok(Some(Exit { pid, handle })),
            true => unwatched(pid, ERROR_INVALID_PARAMETER as i32),
        }
    }

    pub fn wait(self, deadline: Instant) -> Result<(), String> {
        use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::WaitForSingleObject;
        let left = deadline.saturating_duration_since(Instant::now());
        let ms = u32::try_from(left.as_millis()).unwrap_or(u32::MAX - 1);
        match unsafe { WaitForSingleObject(self.handle, ms) } {
            WAIT_OBJECT_0 => Ok(()),
            WAIT_TIMEOUT => Err(not_ended(self.pid)),
            _ => Err(std::io::Error::last_os_error().to_string()),
        }
    }
}

#[cfg(windows)]
impl Drop for Exit {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.handle) };
    }
}
