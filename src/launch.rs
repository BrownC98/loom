use std::fs::{File, TryLockError};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{db, open};

const READY_FOR: Duration = Duration::from_secs(30);
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

    pub fn announce(&self, url: &str, ready_to: Option<SocketAddr>) -> std::io::Result<()> {
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
        std::fs::rename(&tmp, &path)?;
        if let Some(at) = ready_to {
            let _ = TcpStream::connect(at);
        }
        Ok(())
    }
}

pub fn start(listen: Option<SocketAddr>) -> Result<(), String> {
    let url = ensure(listen, crate::protocol::VERSION)?;
    println!("{READY_PREFIX}{url}");
    Ok(())
}

pub fn open(listen: Option<SocketAddr>) -> Result<(), String> {
    let url = ensure(listen, crate::protocol::VERSION)?;
    open::open_url(&url).map_err(|e| format!("could not open the browser: {e}"))?;
    println!("{READY_PREFIX}{url}");
    Ok(())
}

pub fn stop(listen: Option<SocketAddr>) -> Result<(), String> {
    let ours = read_daemon_file().map(|f| f.url);
    let Some(url) = ours
        .clone()
        .or_else(|| listen.map(|at| format!("http://{at}")))
    else {
        println!("loom: no daemon.json. Give --listen to stop a daemon by its address.");
        return Ok(());
    };
    if !shutdown(&url) {
        println!("loom: no daemon runs at {url}.");
        return Ok(());
    }
    if ours.is_none() {
        println!(
            "loom: asked the daemon at {url} to stop. Its data folder has no daemon.json here, so loom does not see its end."
        );
        return Ok(());
    }
    released(Instant::now() + READY_FOR, "the daemon did not stop")?;
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

enum Started {
    Announced,
    Exited,
}

fn ensure(listen: Option<SocketAddr>, mine: &str) -> Result<String, String> {
    let deadline = Instant::now() + READY_FOR;
    let found = read_daemon_file();
    if let Some(f) = &found
        && answers(&f.url)
    {
        if !newer(mine, &f.version) {
            return Ok(f.url.clone());
        }
        shutdown(&f.url);
        released(deadline, "the older daemon did not stop")?;
    }
    let port = found
        .as_ref()
        .and_then(|f| f.url.rsplit_once(':'))
        .and_then(|(_, port)| port.parse().ok())
        .unwrap_or(DEFAULT_LISTEN.port());
    let addr = listen.unwrap_or(SocketAddr::new(DEFAULT_LISTEN.ip(), port));
    let announcements =
        TcpListener::bind((Ipv4Addr::LOCALHOST, ANY_PORT)).map_err(|e| e.to_string())?;
    let ready_to = announcements.local_addr().map_err(|e| e.to_string())?;
    let (tx, started) = mpsc::channel();
    let announced = tx.clone();
    std::thread::spawn(move || {
        for _ in announcements.incoming() {
            if announced.send(Started::Announced).is_err() {
                return;
            }
        }
    });
    let mut daemon = start_daemon(addr, ready_to, tx.clone())?;
    let mut fell_back = addr.port() == ANY_PORT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match started.recv_timeout(left) {
            Ok(Started::Announced) => {
                if let Some(f) = read_daemon_file().filter(|f| f.pid == daemon) {
                    return Ok(f.url);
                }
            }
            Ok(Started::Exited) if !fell_back && lock_free() => {
                daemon = start_daemon(SocketAddr::new(addr.ip(), ANY_PORT), ready_to, tx.clone())?;
                fell_back = true;
            }
            Ok(Started::Exited) if lock_free() => {
                return Err(format!("the daemon stopped. {}", log_hint()));
            }
            Ok(Started::Exited) => {
                return read_daemon_file()
                    .map(|f| f.url)
                    .filter(|url| answers(url))
                    .ok_or_else(|| {
                        format!(
                            "another loom start holds the data folder and its daemon does not answer yet. Run loom start again. {}",
                            log_hint()
                        )
                    });
            }
            Err(_) => {
                return Err(format!(
                    "the daemon did not announce itself in {} seconds. {}",
                    READY_FOR.as_secs(),
                    log_hint()
                ));
            }
        }
    }
}

fn released(deadline: Instant, failure: &str) -> Result<(), String> {
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(data_file("daemon.lock").map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let (tx, taken) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(lock.lock().map(|()| lock));
    });
    let left = deadline.saturating_duration_since(Instant::now());
    match taken.recv_timeout(left) {
        Ok(taken) => taken.map(drop).map_err(|e| e.to_string()),
        Err(_) => Err(format!("{failure} in {} seconds.", READY_FOR.as_secs())),
    }
}

pub fn newer(mine: &str, theirs: &str) -> bool {
    fn parts(version: &str) -> Option<[u64; 3]> {
        let mut numbers = version.split('.').map(|n| n.parse().ok());
        let parts = [numbers.next()??, numbers.next()??, numbers.next()??];
        numbers.next().is_none().then_some(parts)
    }
    matches!((parts(mine), parts(theirs)), (Some(m), Some(t)) if m > t)
}

fn answers(url: &str) -> bool {
    agent().get(url).call().is_ok()
}

fn lock_free() -> bool {
    let Ok(path) = data_file("daemon.lock") else {
        return false;
    };
    let Ok(lock) = File::options().write(true).open(path) else {
        return true;
    };
    lock.try_lock().is_ok()
}

fn shutdown(url: &str) -> bool {
    agent()
        .post(format!("{url}/api/shutdown"))
        .send_empty()
        .is_ok()
}

fn log_hint() -> String {
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

fn start_daemon(
    listen: SocketAddr,
    ready_to: SocketAddr,
    exited: mpsc::Sender<Started>,
) -> Result<u32, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut child = spawn_detached(
        &exe,
        &[
            "serve",
            "--listen",
            &listen.to_string(),
            "--ready-to",
            &ready_to.to_string(),
        ],
    )
    .map_err(|e| format!("could not start the daemon: {e}"))?;
    let pid = child.id();
    std::thread::spawn(move || {
        let _ = child.wait();
        let _ = exited.send(Started::Exited);
    });
    Ok(pid)
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
    crate::timed::daemon_child(&mut cmd);
    cmd.spawn().map_err(|e| format!("{}: {e}", exe.display()))
}
