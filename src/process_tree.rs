//! 기반: OS 프로세스 나무. 중단할 셸과 그 자손을 찾고 끝낸다.
//! 노드 호스트(Claude 셸)와 Codex app-server(Codex 셸)가 함께 쓴다.

use std::collections::HashMap;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

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
        descends(&self.parents, pid, root)
    }

    /// `root`의 자손 가운데 세션 리더(세션 id가 자기 PID)인 프로세스.
    pub fn session_leaders_under(&self, root: u32) -> Vec<u32> {
        self.parents
            .keys()
            .filter(|&&pid| {
                descends(&self.parents, pid, root)
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

/// `pid`가 `root`의 자손이면 참이다. `parents`는 프로세스마다 부모다.
fn descends(parents: &HashMap<u32, u32>, pid: u32, root: u32) -> bool {
    let mut at = pid;
    // 부모 관계에 고리가 있어도 끝나도록 프로세스 수만큼만 올라간다.
    for _ in 0..parents.len() {
        match parents.get(&at) {
            Some(&parent) if parent == root => return true,
            Some(&parent) => at = parent,
            None => return false,
        }
    }
    false
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
    fn subtree_and_descends_follow_parent_links() {
        // 1 ← 10(노드) ← 20(셸) ← 30 ← 40, 10 ← 21(다른 셸), 1 ← 50(남의 프로세스)
        let parents = HashMap::from([(10, 1), (20, 10), (30, 20), (40, 30), (21, 10), (50, 1)]);
        let mut tree = subtree(&parents, 20);
        tree.sort();
        assert_eq!(tree, [20, 30, 40]);
        assert!(descends(&parents, 20, 10));
        assert!(descends(&parents, 40, 10));
        assert!(!descends(&parents, 50, 10));
        assert!(!descends(&parents, 10, 10));
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
