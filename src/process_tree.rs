//! 기반: OS 프로세스. 실행 파일을 찾고, 중단할 셸과 그 자손을 찾아 끝내고, 데몬과 따로 살게 한다.
//! 노드 호스트(Claude 셸), Codex app-server(Codex 셸), OpenCode 2 서버(OpenCode 셸)가 함께 쓴다. OS마다 갈리는 프로세스 처리는 이 파일에만 둔다.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// `PATH`에서 찾은 CLI를 띄우는 명령. 못 찾으면 이름 그대로 띄운다(실행할 때 실패한다).
/// Windows의 `std::process::Command`는 `.exe`만 붙여 찾는다. npm은 CLI를 `.cmd`로 설치한다(예: `codex.cmd`).
pub fn command(name: &str) -> std::process::Command {
    let path = which::which(name).unwrap_or_else(|_| name.into());
    let mut argv = argv(&path).into_iter();
    let mut cmd = std::process::Command::new(argv.next().expect("argv는 비지 않는다"));
    cmd.args(argv);
    cmd
}

/// 실행 파일 `path`를 띄우는 argv 앞부분. 보통은 `[path]`다.
/// npm이 Windows에 설치한 CLI는 배치 파일이다. 배치 파일을 띄우면 `cmd.exe`가 인자의 `<`, `>`, `&`, 줄바꿈을 해석해
/// 사용법 지시와 첫 지시문이 깨진다(실측). 그래서 배치 파일이 부르는 프로그램을 바로 띄운다.
pub fn argv(path: &Path) -> Vec<OsString> {
    npm_shim(path).unwrap_or_else(|| vec![path.into()])
}

/// npm cmd-shim의 끝 줄은 둘 가운데 하나다.
/// - 스크립트: `… "%_prog%"  "%dp0%\..\@openai\codex\bin\codex.js" %*`. `%_prog%`는 배치 파일 옆의
///   `node.exe`가 있으면 그것, 없으면 `PATH`의 `node`다.
/// - 실행 파일: `"%dp0%\..\@opencode\cli\bin\opencode.exe"   %*`. 그 실행 파일을 띄운다.
fn npm_shim(path: &Path) -> Option<Vec<OsString>> {
    if !path.extension()?.eq_ignore_ascii_case("cmd") {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let line = text.lines().rev().find(|l| l.contains("%*"))?;
    let script = line.rsplit_once("\"%dp0%\\")?.1.split('"').next()?;
    let dir = path.parent()?;
    if !line.contains("%_prog%") {
        return Some(vec![dir.join(script).into()]);
    }
    let node = Some(dir.join("node.exe"))
        .filter(|p| p.is_file())
        .or_else(|| which::which("node").ok())?;
    Some(vec![node.into(), dir.join(script).into()])
}

/// 지금 도는 프로세스 전체의 부모 관계. 한 번 읽은 뒤의 판단과 끝내기는 이 사진을 쓴다.
pub struct Snapshot {
    system: System,
    parents: HashMap<u32, u32>,
}

impl Snapshot {
    pub fn now() -> Self {
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        let parents = system
            .processes()
            .iter()
            .filter_map(|(pid, p)| Some((pid.as_u32(), p.parent()?.as_u32())))
            .collect();
        Self { system, parents }
    }

    /// `pid`가 `root`의 자손이면 참이다.
    pub fn descends(&self, pid: u32, root: u32) -> bool {
        child_on_path(&self.parents, root, pid).is_some()
    }

    /// `pid`의 조상(`pid` 자신 포함) 가운데 부모가 `root`인 프로세스. `pid`가 `root`의 자손이 아니면 없다.
    pub fn child_on_path(&self, root: u32, pid: u32) -> Option<u32> {
        child_on_path(&self.parents, root, pid)
    }

    /// `root`의 자손 가운데 세션 리더(세션 id가 자기 PID)인 프로세스.
    pub fn session_leaders_under(&self, root: u32) -> Vec<u32> {
        self.parents
            .keys()
            .filter(|&&pid| {
                child_on_path(&self.parents, root, pid).is_some()
                    && self
                        .system
                        .process(Pid::from_u32(pid))
                        .and_then(|p| p.session_id())
                        .is_some_and(|sid| sid.as_u32() == pid)
            })
            .copied()
            .collect()
    }

    /// `roots`와 그 자손을 모두 끝내고 끝낸 프로세스 수를 돌려준다.
    pub fn kill_trees(&self, roots: impl IntoIterator<Item = u32>) -> usize {
        roots
            .into_iter()
            .flat_map(|root| subtree(&self.parents, root))
            .filter(|&p| {
                self.system
                    .process(Pid::from_u32(p))
                    .is_some_and(|p| p.kill())
            })
            .count()
    }
}

/// `cmd`로 띄운 프로세스가 데몬이 끝나도 살게 한다. 노드의 PTY 호스트와 서버(Codex app-server, OpenCode 2)가 쓴다.
/// Unix: 새 프로세스 그룹에 넣는다. 데몬 터미널의 Ctrl+C·HUP는 데몬의 그룹에만 간다.
/// Windows: 할 일이 없다. 부모가 끝나도 자식은 끝나지 않는다.
#[cfg(unix)]
pub fn detach(cmd: &mut std::process::Command) {
    std::os::unix::process::CommandExt::process_group(cmd, 0);
}

#[cfg(not(unix))]
pub fn detach(_: &mut std::process::Command) {}

/// 서버 프로세스에 넘길 루프백 포트. OS가 고른 빈 포트를 놓고 그 번호를 돌려준다.
/// 서버가 고르게(`--port 0`) 하면 주소를 서버 출력에서 읽어야 한다. 그 파이프는 데몬과 함께 닫혀 서버가 쓰기에 실패한다.
pub fn free_port() -> Result<u16, String> {
    std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| e.to_string())
}

/// `child`가 루프백 `port`에서 들을 때까지 기다린다. 먼저 끝나면 실패한다. 상한은 호출자가 둔다.
pub async fn until_listening(child: &mut tokio::process::Child, port: u16) -> Result<(), String> {
    /// 다시 연결해 보는 간격.
    const POLL: std::time::Duration = std::time::Duration::from_millis(100);
    loop {
        if tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .is_ok()
        {
            return Ok(());
        }
        tokio::select! {
            _ = child.wait() => return Err("exited before listening".into()),
            _ = tokio::time::sleep(POLL) => {}
        }
    }
}

/// 이 가운데 아직 도는 PID만 남긴다.
pub fn alive(pids: &mut Vec<u32>) {
    let list: Vec<Pid> = pids.iter().map(|&p| Pid::from_u32(p)).collect();
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&list),
        true,
        ProcessRefreshKind::nothing(),
    );
    pids.retain(|&p| system.process(Pid::from_u32(p)).is_some());
}

/// `parents`는 프로세스마다 부모다.
fn child_on_path(parents: &HashMap<u32, u32>, root: u32, pid: u32) -> Option<u32> {
    let mut at = pid;
    // 부모 관계에 고리가 있어도 끝나도록 프로세스 수만큼만 올라간다.
    for _ in 0..parents.len() {
        match parents.get(&at) {
            Some(&parent) if parent == root => return Some(at),
            Some(&parent) => at = parent,
            None => return None,
        }
    }
    None
}

/// `root`와 그 자손. `root`가 먼저 온다.
fn subtree(parents: &HashMap<u32, u32>, root: u32) -> Vec<u32> {
    let mut found = vec![root];
    let mut next = 0;
    while next < found.len() {
        let at = found[next];
        let children: Vec<u32> = parents
            .iter()
            .filter(|&(child, &parent)| parent == at && !found.contains(child))
            .map(|(&child, _)| child)
            .collect();
        found.extend(children);
        next += 1;
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtree_and_child_on_path_follow_parent_links() {
        // 1 ← 10(노드) ← 20(셸) ← 30 ← 40, 10 ← 21(다른 셸), 1 ← 50(남의 프로세스)
        let parents = HashMap::from([(10, 1), (20, 10), (30, 20), (40, 30), (21, 10), (50, 1)]);
        let mut tree = subtree(&parents, 20);
        tree.sort();
        assert_eq!(tree, [20, 30, 40]);
        assert_eq!(child_on_path(&parents, 10, 40), Some(20));
        assert_eq!(child_on_path(&parents, 10, 20), Some(20));
        assert_eq!(child_on_path(&parents, 10, 50), None);
    }

    #[test]
    fn npm_shim_runs_its_target_directly() {
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join("codex.cmd");
        std::fs::write(
            &shim,
            "@ECHO off\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n)\r\n\
             endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\..\\@openai\\codex\\bin\\codex.js\" %*\r\n",
        )
        .unwrap();
        let node = dir.path().join("node.exe");
        std::fs::write(&node, "").unwrap();
        assert_eq!(
            argv(&shim),
            [
                node.into_os_string(),
                dir.path().join("..\\@openai\\codex\\bin\\codex.js").into()
            ]
        );
        let opencode = dir.path().join("opencode2.cmd");
        std::fs::write(
            &opencode,
            "@ECHO off\r\nCALL :find_dp0\r\n\"%dp0%\\..\\@opencode\\cli\\bin\\opencode.exe\"   %*\r\n",
        )
        .unwrap();
        assert_eq!(
            argv(&opencode),
            [dir.path()
                .join("..\\@opencode\\cli\\bin\\opencode.exe")
                .into_os_string()]
        );
        let exe = dir.path().join("claude.exe");
        assert_eq!(argv(&exe), [exe.into_os_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn session_leaders_under_skip_children_in_the_parent_session() {
        // `setsid`로 띄운 자식만 세션 리더다. Codex 셸과 MCP 서버를 이렇게 가른다.
        let mut parent = std::process::Command::new("sh")
            .args(["-c", "setsid sleep 30 & sleep 30 & wait"])
            .spawn()
            .unwrap();
        let pid = parent.id();
        let snapshot = (0..50)
            .map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(20));
                Snapshot::now()
            })
            .find(|s| s.parents.values().filter(|&&p| p == pid).count() == 2)
            .unwrap();
        let leaders = snapshot.session_leaders_under(pid);
        assert_eq!(leaders.len(), 1);
        assert_eq!(snapshot.kill_trees(leaders), 1);
        parent.kill().unwrap();
        parent.wait().unwrap();
        Snapshot::now().kill_trees(
            snapshot
                .parents
                .iter()
                .filter(|&(_, &p)| p == pid)
                .map(|(&c, _)| c),
        );
    }
}
