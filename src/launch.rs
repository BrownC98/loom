//! 기계 하나에 데몬 하나. 데이터 폴더(`db::default_path`의 폴더)가 그 자리다.
//! 데몬(`serve`)은 `daemon.lock`을 쥐고 살며, 준비를 마치면 `daemon.json`에 주소·PID·판·실행 파일을 적는다.
//! `loom start`: 데몬이 있으면 붙고, 없으면 띄운다. 도는 데몬이 이 실행 파일보다 옛 판이면 끝내고 이 판으로 띄운다(새 판이 이긴다).
//! `loom open`: `start` 뒤 화면 주소를 기본 브라우저로 연다. 바탕 화면 아이콘이 이 명령을 부른다.
//! `loom stop`: 데몬만 끝낸다. 노드는 산다.
//! 웹 버전과 데스크톱 앱이 모두 `start`를 부른다. 그래서 둘은 같은 데몬을 본다. 근거: docs/design.md 「배포」.

use std::fs::{File, TryLockError};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{db, open};

/// 데몬이 답할 때까지, 또는 옛 데몬이 끝날 때까지 기다리는 상한. 데몬은 시작할 때 내장 스킬을 쓰고 살아 있는 노드에 다시 붙는다.
/// 보통 1초 안에 끝난다. 노드가 많거나 디스크가 느린 첫 시작도 넉넉히 받는다.
const READY_FOR: Duration = Duration::from_secs(30);
/// 다시 물어보는 간격. 사람이 기다림을 느끼지 않을 만큼 짧게 둔다.
const POLL_EVERY: Duration = Duration::from_millis(200);
/// 물음 하나의 상한. 데몬은 주소를 잡은 뒤 준비를 마치고서야 답한다. 그동안 물음이 오래 멈추지 않게 한다.
const PROBE_FOR: Duration = Duration::from_secs(1);
/// 데몬의 기본 주소. 사용자는 한 명이므로 루프백이다. 주소를 주지 않고 `daemon.json`도 없을 때 쓴다.
pub const DEFAULT_LISTEN: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 7420));
/// 그 포트를 다른 프로그램이 쓰면 OS가 고르게 한다.
const ANY_PORT: u16 = 0;
/// `start`가 출력하는 줄의 앞부분. 데스크톱 앱이 이 줄에서 주소를 읽는다(`app/src/main.rs`).
const READY_PREFIX: &str = "loom: ";

/// `<데이터 폴더>/daemon.json`. 도는 데몬을 찾는 유일한 곳이다. 파일이 남았어도 답하지 않으면 데몬이 없다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonFile {
    /// 루프백 주소(`http://127.0.0.1:<포트>`).
    pub url: String,
    pub pid: u32,
    /// 데몬의 제품 판(`crate::VERSION`).
    pub version: String,
    /// 데몬의 실행 파일. macOS `install.sh`가 앱 안의 loom을 찾는다.
    pub exe: PathBuf,
}

/// 데이터 폴더를 쥔 데몬의 잠금. 데몬이 끝나면 OS가 푼다. std는 파일을 CLOEXEC로 열어 PTY 호스트에 넘기지 않는다.
pub struct Claim {
    _lock: File,
}

impl Claim {
    /// 주소를 잡기 전에 부른다. 다른 데몬이 쥐었으면 그 주소로 실패한다.
    pub fn take() -> std::io::Result<Self> {
        let lock = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(data_file("daemon.lock")?)?;
        match lock.try_lock() {
            Ok(()) => Ok(Claim { _lock: lock }),
            Err(TryLockError::WouldBlock) => {
                // 그 데몬이 아직 준비 중이면 `daemon.json`이 없다. 그때는 데이터 폴더를 알린다.
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

    /// 데몬이 답할 준비를 마쳤을 때 부른다. 읽는 쪽이 반쯤 쓴 파일을 보지 않게 다른 이름으로 쓰고 바꾼다.
    pub fn announce(&self, url: &str) -> std::io::Result<()> {
        let file = DaemonFile {
            url: url.to_owned(),
            pid: std::process::id(),
            version: crate::VERSION.to_owned(),
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
    let url = ensure(&mut Host, listen, crate::VERSION)?;
    println!("{READY_PREFIX}{url}");
    Ok(())
}

pub fn open(listen: Option<SocketAddr>) -> Result<(), String> {
    let url = ensure(&mut Host, listen, crate::VERSION)?;
    open::open_url(&url).map_err(|e| format!("could not open the browser: {e}"))?;
    println!("{READY_PREFIX}{url}");
    Ok(())
}

/// `daemon.json`의 데몬을 끝낸다. 파일이 없으면 사람이 준 `listen`(`daemon.json` 전의 데몬)만 끝낸다.
/// 기본 주소로 짐작하지 않는다: 그 주소의 데몬은 다른 데이터 폴더의 것일 수 있다(2026-10-21 시험 `HOME`의 짐작이 사람의 데몬을 끝냈다).
/// 데몬이 다 끝날 때까지 기다린다. 그래야 바로 뒤의 `start`가 끝나는 데몬에 붙지 않는다(`scripts/host.sh`).
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

/// `ensure`가 바깥 세상에 묻는 일. 시험은 가짜로 바꾼다.
trait Machine {
    type Started;
    fn daemon_file(&self) -> Option<DaemonFile>;
    fn answers(&self, url: &str) -> bool;
    /// `daemon.lock`을 쥔 데몬이 없다.
    fn lock_free(&self) -> bool;
    /// 데몬이 끝내기를 받아들였다.
    fn shutdown(&mut self, url: &str) -> bool;
    fn start(&mut self, listen: SocketAddr) -> Result<Self::Started, String>;
    fn exited(&mut self, daemon: &mut Self::Started) -> bool;
    fn wait(&mut self);
    /// 기다림이 실패할 때 알릴 곳.
    fn log_hint(&self) -> String;

    /// 데몬이 다 끝났다: 주소가 답하지 않고 잠금도 풀렸다.
    fn gone(&self, url: &str) -> bool {
        !self.answers(url) && self.lock_free()
    }
}

/// 데몬 하나를 만들고 그 주소를 돌려준다. 근거: 사람 결정 "상태를 하나로 통일하고 프론트는 뷰어의 역할에 충실하게 한다".
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
    // 노드 환경의 `LOOM_URL`은 노드가 끝날 때까지 바뀌지 않는다. 그래서 지난 데몬의 포트를 다시 쓴다.
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
        // 함께 부른 다른 `start`의 데몬이 먼저 잠금을 쥐었으면 이 데몬은 끝나고, 그 데몬이 적은 주소에 붙는다.
        if let Some(f) = m.daemon_file()
            && m.answers(&f.url)
        {
            return Ok(f.url);
        }
        if m.exited(&mut daemon) && m.lock_free() {
            // 잠금을 쥔 데몬이 없는데 끝났다. 포트를 다른 프로그램이 쓴다. OS가 고른 포트로 한 번 더 띄운다.
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

/// `mine`이 `theirs`보다 새 판이다. 판은 `MAJOR.MINOR.PATCH`다(`scripts/version.sh`). 읽지 못하면 같은 판으로 본다.
fn newer(mine: &str, theirs: &str) -> bool {
    fn parts(version: &str) -> Option<[u64; 3]> {
        let mut numbers = version.split('.').map(|n| n.parse().ok());
        let parts = [numbers.next()??, numbers.next()??, numbers.next()??];
        numbers.next().is_none().then_some(parts)
    }
    matches!((parts(mine), parts(theirs)), (Some(m), Some(t)) if m > t)
}

/// 이 기계.
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
        // 파일이 없으면 데몬이 뜬 적이 없다.
        let Ok(lock) = File::options().write(true).open(path) else {
            return true;
        };
        // 쥐었으면 `lock`과 함께 바로 푼다.
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
        // 데몬이 끝나며 남긴 마지막 줄(예: DB가 더 새 판의 것이다)을 함께 보인다.
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
        // 루프백 주소다. 환경의 프록시를 거치면 닿지 않는다.
        .proxy(None)
        .build()
        .into()
}

/// 이 실행 파일로 `loom serve`를 띄운다. 부른 터미널·바로 가기·앱이 끝나도 산다. 출력은 `serve.log`에 붙인다.
fn start_daemon(listen: SocketAddr) -> Result<Child, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let log_path = data_file("serve.log").map_err(|e| e.to_string())?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("{}: {e}", log_path.display()))?;
    let mut cmd = Command::new(exe);
    cmd.args(["serve", "--listen", &listen.to_string()])
        // 데몬은 시작 폴더가 git 저장소면 첫 저장소로 더한다. 바로 가기·앱에는 고른 폴더가 없다.
        // 루트는 저장소가 아니다. 홈은 dotfiles 저장소일 수 있다. 사람은 화면의 [+]로 저장소를 더한다.
        .current_dir(Path::new(std::path::MAIN_SEPARATOR_STR))
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    detach(&mut cmd);
    cmd.spawn()
        .map_err(|e| format!("could not start the daemon: {e}"))
}

#[cfg(unix)]
fn detach(cmd: &mut Command) {
    crate::process_tree::new_session(cmd);
}

/// 부모가 끝나도 자식은 산다. 숨은 콘솔을 준다: 데몬이 띄우는 콘솔 프로그램(git, PTY 호스트)이 그 콘솔을 물려받아 새 창을 열지 않는다.
#[cfg(windows)]
fn detach(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    const OLD: &str = "http://127.0.0.1:7420";

    fn file(url: &str, version: &str) -> DaemonFile {
        DaemonFile {
            url: url.into(),
            pid: 1,
            version: version.into(),
            exe: "/loom".into(),
        }
    }

    /// 대본대로 답하는 기계. `answering`은 지금 답하는 주소다. 띄운 데몬은 `ready_after`번 기다린 뒤 답한다.
    #[derive(Default)]
    struct Fake {
        file: Option<DaemonFile>,
        answering: Vec<String>,
        /// 끝내기를 받는 주소(loom). 받으면 그 주소는 답하지 않는다.
        loom_at: Vec<String>,
        /// 띄운 데몬마다 결과: `Some(port)`는 그 포트에서 뜬다, `None`은 바로 끝난다.
        outcomes: VecDeque<Option<u16>>,
        started: Vec<SocketAddr>,
        shutdowns: Vec<String>,
        locked: bool,
        pending: Option<(u16, u32)>,
        ready_after: u32,
    }

    impl Machine for Fake {
        type Started = bool;
        fn daemon_file(&self) -> Option<DaemonFile> {
            self.file.clone()
        }
        fn answers(&self, url: &str) -> bool {
            self.answering.iter().any(|a| a == url)
        }
        fn lock_free(&self) -> bool {
            !self.locked
        }
        fn shutdown(&mut self, url: &str) -> bool {
            self.shutdowns.push(url.into());
            if !self.loom_at.iter().any(|a| a == url) {
                return false;
            }
            self.answering.retain(|a| a != url);
            self.locked = false;
            true
        }
        fn start(&mut self, listen: SocketAddr) -> Result<bool, String> {
            self.started.push(listen);
            match self.outcomes.pop_front().flatten() {
                Some(port) => {
                    self.locked = true;
                    self.pending = Some((port, self.ready_after));
                    Ok(false)
                }
                None => Ok(true),
            }
        }
        fn exited(&mut self, daemon: &mut bool) -> bool {
            *daemon
        }
        fn wait(&mut self) {
            if let Some((port, left)) = self.pending.take() {
                if left == 0 {
                    let url = format!("http://127.0.0.1:{port}");
                    self.file = Some(file(&url, "0.1.9"));
                    self.answering.push(url);
                } else {
                    self.pending = Some((port, left - 1));
                }
            }
        }
        fn log_hint(&self) -> String {
            "(see log)".into()
        }
    }

    fn at(port: u16) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, port))
    }

    #[test]
    fn same_or_newer_daemon_is_attached() {
        for theirs in ["0.1.9", "0.2.0", "dev"] {
            let mut m = Fake {
                file: Some(file(OLD, theirs)),
                answering: vec![OLD.into()],
                locked: true,
                ..Fake::default()
            };
            assert_eq!(ensure(&mut m, None, "0.1.9"), Ok(OLD.into()));
            assert!(m.started.is_empty() && m.shutdowns.is_empty());
        }
    }

    #[test]
    fn older_daemon_is_replaced_on_its_port() {
        let mut m = Fake {
            file: Some(file("http://127.0.0.1:7555", "0.1.5")),
            answering: vec!["http://127.0.0.1:7555".into()],
            loom_at: vec!["http://127.0.0.1:7555".into()],
            locked: true,
            outcomes: [Some(7555)].into(),
            ready_after: 2,
            ..Fake::default()
        };
        assert_eq!(
            ensure(&mut m, None, "0.1.9"),
            Ok("http://127.0.0.1:7555".into())
        );
        assert_eq!(m.shutdowns, ["http://127.0.0.1:7555"]);
        assert_eq!(m.started, [at(7555)]);
    }

    #[test]
    fn stale_file_starts_a_daemon_on_its_port() {
        let mut m = Fake {
            file: Some(file("http://127.0.0.1:7555", "0.1.9")),
            outcomes: [Some(7555)].into(),
            ..Fake::default()
        };
        assert_eq!(
            ensure(&mut m, None, "0.1.9"),
            Ok("http://127.0.0.1:7555".into())
        );
        assert_eq!(m.started, [at(7555)]);
    }

    #[test]
    fn no_file_starts_on_listen_or_default() {
        let mut m = Fake {
            outcomes: [Some(7420)].into(),
            ..Fake::default()
        };
        assert_eq!(ensure(&mut m, None, "0.1.9"), Ok(OLD.into()));
        assert_eq!(m.started, [at(7420)]);

        let mut m = Fake {
            outcomes: [Some(7600)].into(),
            ..Fake::default()
        };
        ensure(&mut m, Some(at(7600)), "0.1.9").unwrap();
        assert_eq!(m.started, [at(7600)]);
    }

    #[test]
    fn taken_port_falls_back_to_any_port() {
        let mut m = Fake {
            // 다른 프로그램이 7420에서 답한다. `daemon.json`에 없는 데몬은 다른 데이터 폴더의 것일 수 있어 끝내지 않는다.
            answering: vec![OLD.into()],
            loom_at: vec![OLD.into()],
            outcomes: [None, Some(40001)].into(),
            ..Fake::default()
        };
        assert_eq!(
            ensure(&mut m, None, "0.1.9"),
            Ok("http://127.0.0.1:40001".into())
        );
        assert_eq!(m.started, [at(7420), at(ANY_PORT)]);
        assert!(m.shutdowns.is_empty());
    }

    #[test]
    fn daemons_not_in_our_file_are_never_stopped() {
        // 다른 데이터 폴더의 데몬이 기본 포트와 다른 포트에서 돈다. 이 데이터 폴더에는 `daemon.json`이 없다.
        let other = "http://127.0.0.1:7555";
        let mut m = Fake {
            answering: vec![OLD.into(), other.into()],
            loom_at: vec![OLD.into(), other.into()],
            outcomes: [None, Some(40002)].into(),
            ..Fake::default()
        };
        assert_eq!(
            ensure(&mut m, None, "9.9.9"),
            Ok("http://127.0.0.1:40002".into())
        );
        assert!(m.shutdowns.is_empty());
        assert!(m.answers(OLD) && m.answers(other));
    }

    #[test]
    fn lost_race_attaches_to_the_winner() {
        // 이 데몬은 끝났지만 함께 띄운 다른 데몬이 잠금을 쥐고 준비 중이다. 다시 띄우지 않고 기다린다.
        let mut m = Fake {
            outcomes: [None].into(),
            locked: true,
            pending: Some((7420, 1)),
            ..Fake::default()
        };
        assert_eq!(ensure(&mut m, None, "0.1.9"), Ok(OLD.into()));
        assert_eq!(m.started, [at(7420)]);
    }

    #[test]
    fn daemon_that_keeps_stopping_is_reported() {
        let mut m = Fake {
            outcomes: [None, None].into(),
            ..Fake::default()
        };
        assert_eq!(
            ensure(&mut m, None, "0.1.9"),
            Err("the daemon stopped. (see log)".into())
        );
        assert_eq!(m.started.len(), 2);
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(newer("0.1.10", "0.1.9"));
        assert!(newer("1.0.0", "0.9.99"));
        assert!(!newer("0.1.9", "0.1.9"));
        assert!(!newer("0.1.5", "0.1.9"));
        assert!(!newer("0.1.9", "0.1"));
        assert!(!newer("0.1.9", "0.1.5-dev"));
    }
}
