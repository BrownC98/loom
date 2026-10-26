use std::fs::{File, TryLockError};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{db, open};

const READY_FOR: Duration = Duration::from_secs(30);
const POLL_EVERY: Duration = Duration::from_millis(200);
const PROBE_FOR: Duration = Duration::from_secs(1);
pub const DEFAULT_LISTEN: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 7420));
const ANY_PORT: u16 = 0;
const READY_PREFIX: &str = "loom: ";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonFile {
    pub url: String,
    pub pid: u32,
    pub version: String,
    pub exe: PathBuf,
}

pub struct Claim {
    _lock: File,
}

impl Claim {
    pub fn take() -> std::io::Result<Self> {
        let lock = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(data_file("daemon.lock")?)?;
        match lock.try_lock() {
            Ok(()) => Ok(Claim { _lock: lock }),
            Err(TryLockError::WouldBlock) => {
                let at = match read_daemon_file() {
                    Some(f) => f.url,
                    None => db::default_path()?
                        .parent()
                        .unwrap_or(Path::new(""))
                        .display()
                        .to_string(),
                };
                Err(std::io::Error::other(format!(
                    "a daemon already runs at {at}"
                )))
            }
            Err(TryLockError::Error(e)) => Err(e),
        }
    }

    pub fn announce(&self, url: &str) -> std::io::Result<()> {
        let file = DaemonFile {
            url: url.to_owned(),
            pid: std::process::id(),
            version: crate::protocol::VERSION.to_owned(),
            exe: std::env::current_exe()?,
        };
        let path = data_file("daemon.json")?;
        let tmp = path.with_extension("json.new");
        std::fs::write(
            &tmp,
            serde_json::to_vec(&file).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(&tmp, &path)
    }
}

pub fn start(listen: Option<SocketAddr>) -> Result<(), String> {
    let url = ensure(&mut Host, listen, crate::protocol::VERSION)?;
    println!("{READY_PREFIX}{url}");
    Ok(())
}

pub fn open(listen: Option<SocketAddr>) -> Result<(), String> {
    let url = ensure(&mut Host, listen, crate::protocol::VERSION)?;
    open::open_url(&url).map_err(|e| format!("could not open the browser: {e}"))?;
    println!("{READY_PREFIX}{url}");
    Ok(())
}

pub fn stop(listen: Option<SocketAddr>) -> Result<(), String> {
    let mut host = Host;
    let Some(url) = read_daemon_file()
        .map(|f| f.url)
        .or_else(|| listen.map(|at| format!("http://{at}")))
    else {
        println!("loom: no daemon.json. Give --listen to stop a daemon by its address.");
        return Ok(());
    };
    if !host.shutdown(&url) {
        println!("loom: no daemon runs at {url}.");
        return Ok(());
    }
    until(&mut host, |h| h.gone(&url), "the daemon did not stop")?;
    println!("loom: stopped the daemon at {url}. Nodes keep running.");
    Ok(())
}

pub fn carry(from: &Path) -> Result<(), String> {
    let url = read_daemon_file()
        .map(|f| f.url)
        .ok_or("loom: no daemon runs. Run `loom start` first.")?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .proxy(None)
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent
        .post(format!("{url}/api/carry"))
        .send_json(serde_json::json!({ "from": from.to_string_lossy() }))
        .map_err(|e| e.to_string())?;
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("loom: carry failed: {text}"));
    }
    println!("loom: {text}");
    Ok(())
}

trait Machine {
    type Started;
    fn daemon_file(&self) -> Option<DaemonFile>;
    fn answers(&self, url: &str) -> bool;
    fn lock_free(&self) -> bool;
    fn shutdown(&mut self, url: &str) -> bool;
    fn start(&mut self, listen: SocketAddr) -> Result<Self::Started, String>;
    fn exited(&mut self, daemon: &mut Self::Started) -> bool;
    fn wait(&mut self);
    fn log_hint(&self) -> String;

    fn gone(&self, url: &str) -> bool {
        !self.answers(url) && self.lock_free()
    }
}

fn ensure<M: Machine>(m: &mut M, listen: Option<SocketAddr>, mine: &str) -> Result<String, String> {
    let found = m.daemon_file();
    if let Some(f) = &found
        && m.answers(&f.url)
    {
        if !newer(mine, &f.version) {
            return Ok(f.url.clone());
        }
        m.shutdown(&f.url);
        until(m, |m| m.gone(&f.url), "the older daemon did not stop")?;
    }
    let port = found
        .as_ref()
        .and_then(|f| f.url.rsplit_once(':'))
        .and_then(|(_, port)| port.parse().ok())
        .unwrap_or(DEFAULT_LISTEN.port());
    let addr = listen.unwrap_or(SocketAddr::new(DEFAULT_LISTEN.ip(), port));
    let mut daemon = m.start(addr)?;
    let mut fell_back = addr.port() == ANY_PORT;
    let deadline = Instant::now() + READY_FOR;
    while Instant::now() < deadline {
        if let Some(f) = m.daemon_file()
            && m.answers(&f.url)
        {
            return Ok(f.url);
        }
        if m.exited(&mut daemon) && m.lock_free() {
            if fell_back {
                return Err(format!("the daemon stopped. {}", m.log_hint()));
            }
            daemon = m.start(SocketAddr::new(addr.ip(), ANY_PORT))?;
            fell_back = true;
        }
        m.wait();
    }
    Err(format!(
        "the daemon did not answer in {} seconds. {}",
        READY_FOR.as_secs(),
        m.log_hint()
    ))
}

fn until<M: Machine>(m: &mut M, done: impl Fn(&M) -> bool, failure: &str) -> Result<(), String> {
    let deadline = Instant::now() + READY_FOR;
    while !done(m) {
        if Instant::now() >= deadline {
            return Err(format!("{failure} in {} seconds.", READY_FOR.as_secs()));
        }
        m.wait();
    }
    Ok(())
}

pub fn newer(mine: &str, theirs: &str) -> bool {
    fn parts(version: &str) -> Option<[u64; 3]> {
        let mut numbers = version.split('.').map(|n| n.parse().ok());
        let parts = [numbers.next()??, numbers.next()??, numbers.next()??];
        numbers.next().is_none().then_some(parts)
    }
    matches!((parts(mine), parts(theirs)), (Some(m), Some(t)) if m > t)
}

struct Host;

impl Machine for Host {
    type Started = Child;

    fn daemon_file(&self) -> Option<DaemonFile> {
        read_daemon_file()
    }

    fn answers(&self, url: &str) -> bool {
        agent().get(url).call().is_ok()
    }

    fn lock_free(&self) -> bool {
        let Ok(path) = data_file("daemon.lock") else {
            return false;
        };
        let Ok(lock) = File::options().write(true).open(path) else {
            return true;
        };
        lock.try_lock().is_ok()
    }

    fn shutdown(&mut self, url: &str) -> bool {
        agent()
            .post(format!("{url}/api/shutdown"))
            .send_empty()
            .is_ok()
    }

    fn start(&mut self, listen: SocketAddr) -> Result<Child, String> {
        start_daemon(listen)
    }

    fn exited(&mut self, daemon: &mut Child) -> bool {
        !matches!(daemon.try_wait(), Ok(None))
    }

    fn wait(&mut self) {
        std::thread::sleep(POLL_EVERY);
    }

    fn log_hint(&self) -> String {
        let Ok(path) = data_file("serve.log") else {
            return String::new();
        };
        let last = std::fs::read_to_string(&path)
            .ok()
            .and_then(|log| {
                let last = log.lines().last()?;
                Some(last.strip_prefix(READY_PREFIX).unwrap_or(last).to_owned())
            })
            .unwrap_or_default();
        format!("{last} (see {})", path.display())
    }
}

fn read_daemon_file() -> Option<DaemonFile> {
    let text = std::fs::read(data_file("daemon.json").ok()?).ok()?;
    serde_json::from_slice(&text).ok()
}

fn data_file(name: &str) -> std::io::Result<PathBuf> {
    Ok(db::default_path()?.with_file_name(name))
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(PROBE_FOR))
        .proxy(None)
        .build()
        .into()
}

fn start_daemon(listen: SocketAddr) -> Result<Child, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    spawn_detached(&exe, &["serve", "--listen", &listen.to_string()])
        .map_err(|e| format!("could not start the daemon: {e}"))
}

pub fn restart_with(exe: &Path, this_daemon: SocketAddr) -> Result<(), String> {
    spawn_detached(exe, &["start", "--listen", &this_daemon.to_string()]).map(drop)
}

pub fn spawn_detached(exe: &Path, args: &[&str]) -> Result<Child, String> {
    let log_path = data_file("serve.log").map_err(|e| e.to_string())?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("{}: {e}", log_path.display()))?;
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .current_dir(Path::new(std::path::MAIN_SEPARATOR_STR))
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    detach(&mut cmd);
    cmd.spawn().map_err(|e| format!("{}: {e}", exe.display()))
}

#[cfg(unix)]
fn detach(cmd: &mut Command) {
    crate::process_tree::new_session(cmd);
}

#[cfg(windows)]
fn detach(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const STD_HANDLES: [u32; 3] = [-10i32 as u32, -11i32 as u32, -12i32 as u32];
    const HANDLE_FLAG_INHERIT: u32 = 1;
    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> isize;
        fn SetHandleInformation(handle: isize, mask: u32, flags: u32) -> i32;
    }
    for which in STD_HANDLES {
        unsafe { SetHandleInformation(GetStdHandle(which), HANDLE_FLAG_INHERIT, 0) };
    }
    cmd.creation_flags(CREATE_NO_WINDOW);
}
