//! `loom open`: 데몬이 없으면 띄우고, 화면 주소를 기본 브라우저로 연다. 바탕 화면 아이콘이 이 명령을 부른다.
//! `loom stop`: 데몬만 끝낸다. 노드는 산다.
//! 근거: docs/design.md 「배포」.

use std::net::SocketAddr;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::{db, open, process_tree};

/// 데몬이 답할 때까지 기다리는 상한. 데몬은 시작할 때 내장 스킬을 쓰고 살아 있는 노드에 다시 붙는다. 보통 1초 안에 끝난다.
/// 노드가 많거나 디스크가 느린 첫 시작도 넉넉히 받는다.
const READY_FOR: Duration = Duration::from_secs(30);
/// 다시 물어보는 간격. 사람이 기다림을 느끼지 않을 만큼 짧게 둔다.
const POLL_EVERY: Duration = Duration::from_millis(200);
/// 물음 하나의 상한. 데몬은 주소를 잡은 뒤 준비를 마치고서야 답한다. 그동안 물음이 오래 멈추지 않게 한다.
const PROBE_FOR: Duration = Duration::from_secs(1);

pub fn open(listen: SocketAddr) -> Result<(), String> {
    let url = format!("http://{listen}");
    let started = ensure_daemon(|| answers(&url), || start_daemon(listen))?;
    if let Some(mut daemon) = started {
        // 같이 누른 다른 `loom open`의 데몬이 주소를 먼저 잡았으면 이 데몬은 이미 끝났다. 산 데몬만 적는다.
        if daemon.try_wait().map_err(|e| e.to_string())?.is_none() {
            let pid_file = db::default_path()
                .map_err(|e| e.to_string())?
                .with_file_name("serve.pid");
            std::fs::write(&pid_file, daemon.id().to_string())
                .map_err(|e| format!("{}: {e}", pid_file.display()))?;
        }
    }
    open::open_url(&url).map_err(|e| format!("could not open the browser: {e}"))?;
    println!("loom: {url}");
    Ok(())
}

pub fn stop(listen: SocketAddr) -> Result<(), String> {
    let url = format!("http://{listen}/api/shutdown");
    match agent().post(&url).send_empty() {
        Ok(_) => println!("loom: stopped the daemon at {listen}. Nodes keep running."),
        Err(ureq::Error::Io(_) | ureq::Error::ConnectionFailed) => {
            println!("loom: no daemon runs at {listen}.")
        }
        Err(e) => return Err(format!("could not stop the daemon: {e}")),
    }
    Ok(())
}

/// 데몬이 답하지 않으면 한 번 띄우고 답할 때까지 기다린다. 띄웠으면 그 프로세스를 돌려준다.
/// 둘이 함께 띄우면 늦은 데몬은 주소를 잡지 못해 끝난다. 둘 다 먼저 뜬 데몬의 답을 받는다.
fn ensure_daemon<P>(
    answers: impl Fn() -> bool,
    start: impl FnOnce() -> Result<P, String>,
) -> Result<Option<P>, String> {
    if answers() {
        return Ok(None);
    }
    let daemon = start()?;
    let deadline = Instant::now() + READY_FOR;
    while !answers() {
        if Instant::now() >= deadline {
            return Err(format!(
                "the daemon did not answer in {} seconds. See {}.",
                READY_FOR.as_secs(),
                log_path()?.display()
            ));
        }
        std::thread::sleep(POLL_EVERY);
    }
    Ok(Some(daemon))
}

fn answers(url: &str) -> bool {
    agent().get(url).call().is_ok()
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(PROBE_FOR))
        // 루프백 주소다. 환경의 프록시를 거치면 닿지 않는다.
        .proxy(None)
        .build()
        .into()
}

/// 이 실행 파일로 `loom serve`를 새 세션에 띄운다. 부른 터미널·바로 가기가 끝나도 산다. 출력은 `serve.log`에 붙인다.
fn start_daemon(listen: SocketAddr) -> Result<Child, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let log_path = log_path()?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("{}: {e}", log_path.display()))?;
    let mut cmd = Command::new(exe);
    cmd.args(["serve", "--listen", &listen.to_string()])
        // 데몬은 시작 폴더가 git 저장소면 첫 저장소로 더한다. 바로 가기에는 고른 폴더가 없다.
        // 루트는 저장소가 아니다. 홈은 dotfiles 저장소일 수 있다. 사람은 화면의 [+]로 저장소를 더한다.
        .current_dir(Path::new(std::path::MAIN_SEPARATOR_STR))
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    process_tree::new_session(&mut cmd);
    cmd.spawn()
        .map_err(|e| format!("could not start the daemon: {e}"))
}

fn log_path() -> Result<std::path::PathBuf, String> {
    Ok(db::default_path()
        .map_err(|e| e.to_string())?
        .with_file_name("serve.log"))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn running_daemon_is_not_started_again() {
        let started = Cell::new(0);
        let result = ensure_daemon(
            || true,
            || {
                started.set(started.get() + 1);
                Ok(())
            },
        );
        assert_eq!(result, Ok(None));
        assert_eq!(started.get(), 0);
    }

    #[test]
    fn missing_daemon_is_started_once_and_awaited() {
        let started = Cell::new(0);
        let asked = Cell::new(0);
        // 띄우기 전 한 번, 띄운 뒤 두 번 답하지 않다가 답한다.
        let result = ensure_daemon(
            || {
                asked.set(asked.get() + 1);
                asked.get() > 3
            },
            || {
                started.set(started.get() + 1);
                Ok("daemon")
            },
        );
        assert_eq!(result, Ok(Some("daemon")));
        assert_eq!(started.get(), 1);
        assert_eq!(asked.get(), 4);
    }

    #[test]
    fn failed_start_is_reported() {
        let result = ensure_daemon(|| false, || Err::<(), _>("no exe".to_owned()));
        assert_eq!(result, Err("no exe".to_owned()));
    }
}
