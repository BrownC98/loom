use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

pub fn command(name: &str) -> std::process::Command {
    command_at(&which::which(name).unwrap_or_else(|_| name.into()))
}

pub fn command_at(path: &Path) -> std::process::Command {
    let mut argv = argv(path).into_iter();
    let mut cmd = std::process::Command::new(argv.next().expect("argv는 비지 않는다"));
    cmd.args(argv);
    cmd
}

pub fn argv(path: &Path) -> Vec<OsString> {
    npm_shim(path).unwrap_or_else(|| vec![path.into()])
}

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

    pub fn descends(&self, pid: u32, root: u32) -> bool {
        child_on_path(&self.parents, root, pid).is_some()
    }

    pub fn child_on_path(&self, root: u32, pid: u32) -> Option<u32> {
        child_on_path(&self.parents, root, pid)
    }

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

    pub fn runs(&self, root: u32, name: &str) -> bool {
        subtree(&self.parents, root).into_iter().any(|pid| {
            self.system
                .process(Pid::from_u32(pid))
                .is_some_and(|p| p.name() == name)
        })
    }

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

#[cfg(unix)]
pub fn detach(cmd: &mut std::process::Command) {
    std::os::unix::process::CommandExt::process_group(cmd, 0);
}

#[cfg(not(unix))]
pub fn detach(_: &mut std::process::Command) {}

#[cfg(unix)]
pub fn new_session(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(windows)]
pub fn new_session(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    cmd.creation_flags(DETACHED_PROCESS);
}

pub fn free_port() -> Result<u16, String> {
    std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| e.to_string())
}

pub async fn until_listening(child: &mut tokio::process::Child, port: u16) -> Result<(), String> {
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

fn child_on_path(parents: &HashMap<u32, u32>, root: u32, pid: u32) -> Option<u32> {
    let mut at = pid;
    for _ in 0..parents.len() {
        match parents.get(&at) {
            Some(&parent) if parent == root => return Some(at),
            Some(&parent) => at = parent,
            None => return None,
        }
    }
    None
}

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
        let parents = HashMap::from([(10, 1), (20, 10), (30, 20), (40, 30), (21, 10), (50, 1)]);
        let mut tree = subtree(&parents, 20);
        tree.sort();
        assert_eq!(tree, [20, 30, 40]);
        assert_eq!(child_on_path(&parents, 10, 40), Some(20));
        assert_eq!(child_on_path(&parents, 10, 20), Some(20));
        assert_eq!(child_on_path(&parents, 10, 50), None);
    }

    #[cfg(unix)]
    #[test]
    fn runs_finds_a_process_name_in_the_tree() {
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .unwrap();
        let snapshot = Snapshot::now();
        let me = std::process::id();
        assert!(snapshot.runs(me, "sleep"));
        assert!(!snapshot.runs(child.id(), "loom"));
        child.kill().unwrap();
        child.wait().unwrap();
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
        let mut parent = std::process::Command::new("sh")
            .args([
                "-c",
                "perl -MPOSIX -e 'setsid; exec @ARGV' sleep 30 & sleep 30 & wait",
            ])
            .spawn()
            .unwrap();
        let pid = parent.id();
        let snapshot = (0..50)
            .map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(20));
                Snapshot::now()
            })
            .find(|s| {
                s.parents
                    .iter()
                    .filter(|&(&child, &p)| {
                        p == pid
                            && s.system
                                .process(Pid::from_u32(child))
                                .is_some_and(|c| c.name() == "sleep")
                    })
                    .count()
                    == 2
            })
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
