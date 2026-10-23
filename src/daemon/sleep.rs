//! 잠든 노드: 완료한 뒤 쉬는 자식 노드의 프로세스를 끝내고, 메시지나 사람 입력이 오면 같은 세션으로 깨운다. 근거: docs/design.md 「잠든 노드」.

use std::collections::{HashMap, HashSet};
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

use super::App;
use super::relaunch::{EXIT_WAIT, Plan, profile_of};
use crate::graph::{Active, NodeId};
use crate::process_tree::Snapshot;
use crate::profile::{Profile, ServerLink};

/// 완료한 뒤 쉬는 노드를 재우기까지 기다리는 시간.
/// 근거: 사람 DB에서 완료 뒤 다시 일한 476번 가운데 90%가 10분 안에 왔다(중앙값 75초). 352번은 다시 일하지 않았다(2026-10-24 실측).
const DONE_GRACE: Duration = Duration::from_secs(600);
/// 쉬는 노드를 다시 보는 간격. 메시지와 사람 입력은 이 간격을 기다리지 않고 바로 깨운다.
const SWEEP: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct Sleeps {
    /// 쉬는 완료 노드와 쉬기 시작한 때. 한 번이라도 쉬지 않으면 다시 잰다.
    resting: Mutex<HashMap<NodeId, Instant>>,
    /// 재우는 중인 노드. 프로세스가 끝날 때까지 메시지를 꺼내지 않는다(`unless_stopping`). 남은 메시지는 깨울 때 받는다.
    stopping: Mutex<HashSet<NodeId>>,
    /// 사람이 터미널에 키를 누른 노드. 잠들었으면 깨운다.
    knocked: Mutex<HashSet<NodeId>>,
    knock: Notify,
}

impl Sleeps {
    pub fn stopping(&self, node: NodeId) -> bool {
        self.stopping.lock().unwrap().contains(&node)
    }

    /// 재우는 중이 아니면 `take`로 메시지를 꺼낸다. 재우기 표시(`begin`)와 같은 잠금 안에서 꺼낸다.
    /// 그래서 표시 전에 꺼낸 메시지는 재우기의 다시 보기가 보고(노드가 `running`이다), 표시 뒤에는 꺼내지 않는다.
    pub fn unless_stopping<T>(&self, node: NodeId, take: impl FnOnce() -> T) -> Option<T> {
        let stopping = self.stopping.lock().unwrap();
        (!stopping.contains(&node)).then(take)
    }

    /// 재우기를 시작한다. 이미 재우는 중이면 거짓이다.
    fn begin(&self, node: NodeId) -> bool {
        self.stopping.lock().unwrap().insert(node)
    }

    /// 재우기를 멈췄다. 재우는 중이었으면 참이다.
    fn cancel(&self, node: NodeId) -> bool {
        self.stopping.lock().unwrap().remove(&node)
    }

    pub fn exited(&self, node: NodeId) {
        self.stopping.lock().unwrap().remove(&node);
        self.resting.lock().unwrap().remove(&node);
    }

    pub fn knocked(&self, node: NodeId) {
        self.knocked.lock().unwrap().insert(node);
        self.knock.notify_one();
    }

    /// 지금 쉬는 노드(`resting`) 가운데 `DONE_GRACE` 동안 쉰 노드.
    fn due(&self, resting: &[NodeId], now: Instant) -> Vec<NodeId> {
        let mut since = self.resting.lock().unwrap();
        since.retain(|id, _| resting.contains(id));
        resting
            .iter()
            .filter(|&&id| now.duration_since(*since.entry(id).or_insert(now)) >= DONE_GRACE)
            .copied()
            .collect()
    }
}

impl App {
    /// 잠든 노드를 깨우고 쉬는 노드를 재운다. 데몬이 시작할 때 한 번 부른다.
    pub(super) fn watch_sleep(self: &Arc<Self>) {
        let app = self.clone();
        tokio::spawn(async move {
            let mut next_sweep = Instant::now();
            loop {
                let mut mail = pin!(app.graph.mail_changed());
                mail.as_mut().enable();
                let mut knock = pin!(app.sleeps.knock.notified());
                knock.as_mut().enable();
                if let Err(e) = app.wake_sleepers() {
                    eprintln!("loom: 잠든 노드를 보지 못했다: {e}");
                }
                if Instant::now() >= next_sweep {
                    if let Err(e) = app.sleep_resting() {
                        eprintln!("loom: 쉬는 노드를 보지 못했다: {e}");
                    }
                    next_sweep = Instant::now() + SWEEP;
                }
                tokio::select! {
                    () = mail => {}
                    () = knock => {}
                    () = tokio::time::sleep_until(next_sweep.into()) => {}
                }
            }
        });
    }

    /// 깨우는 메시지가 왔거나 사람이 키를 누른 잠든 노드를 다시 띄우기와 같은 길로 깨운다. 깨우지 못하면 `exited`로 바꾼다.
    fn wake_sleepers(self: &Arc<Self>) -> rusqlite::Result<()> {
        let knocked = std::mem::take(&mut *self.sleeps.knocked.lock().unwrap());
        let mut waking = self.graph.sleepers_with_mail()?;
        for id in knocked {
            if !waking.contains(&id) && self.graph.asleep(id)? {
                waking.push(id);
            }
        }
        for id in waking {
            let app = self.clone();
            tokio::spawn(async move {
                if let Err(e) = app.relaunch_by_id(id, Plan::Resume).await {
                    eprintln!("loom: 잠든 노드 {}를 깨우지 못했다: {e:?}", id.0);
                    if let Err(e) = app.graph.sleep_lost(id) {
                        eprintln!("loom: 노드 {}를 exited로 바꾸지 못했다: {e}", id.0);
                    }
                }
            });
        }
        Ok(())
    }

    /// 쉬는 완료 자식을 `DONE_GRACE` 뒤에 재운다.
    fn sleep_resting(self: &Arc<Self>) -> rusqlite::Result<()> {
        let children = self.graph.done_children()?;
        let resting: Vec<Active> = self
            .graph
            .active()?
            .into_iter()
            .filter(|n| children.contains(&n.id) && self.resting(n))
            .collect();
        let ids: Vec<NodeId> = resting.iter().map(|n| n.id).collect();
        let due = self.sleeps.due(&ids, Instant::now());
        for node in resting {
            if due.contains(&node.id) && self.sleeps.begin(node.id) {
                let app = self.clone();
                tokio::spawn(async move { app.put_to_sleep(node).await });
            }
        }
        Ok(())
    }

    /// 세션을 이을 수 있고 턴이 없고 다시 띄울 일(`relaunch_stale`)이 없는 노드. Claude·Cursor는 쉬는 대기가 열려 있어야 한다.
    /// 다시 시작한 데몬은 훅이 다시 붙을 때까지 기다린다.
    fn resting(&self, node: &Active) -> bool {
        if self.relaunches.holds_mail(node.id) {
            return false;
        }
        match profile_of(node) {
            Some(Profile::Claude | Profile::Cursor) => {
                node.transcript.is_some() && self.idle.waiting(node.id)
            }
            Some(Profile::Codex) => {
                matches!(
                    node.server,
                    Some(ServerLink::Codex {
                        thread: Some(_),
                        ..
                    })
                ) && !self.busy(node.id)
            }
            // OpenCode 2는 재우지 않는다: 깨운 뒤 메시지를 받고 `done`을 불렀는데, loom이 넣지 않은 턴이 하나 더 돌아
            // 사람 입력으로 보여 노드가 `running`으로 돌아갔다(2026-10-24 시험 데몬, OpenCode 2 노드 이벤트 69). 카드가 완료로 남지 않는다.
            Some(Profile::OpenCode2) | None => false,
        }
    }

    /// 프로세스와 그 백그라운드 명령을 끝낸다. 노드는 `done`으로 남고(`Graph::slept`), 메시지를 계속 받는다(`REACHABLE`).
    /// 메시지는 `begin` 뒤로 꺼내지 않는다. 그 전에 다시 일했으면 재우지 않는다.
    /// 프로세스가 `EXIT_WAIT` 안에 끝나지 않으면 재우기를 멈춰 산 노드가 다시 메시지를 받게 한다.
    async fn put_to_sleep(&self, node: Active) {
        let id = node.id;
        let still = || {
            self.graph
                .done_children()
                .is_ok_and(|done| done.contains(&id))
                && !self.busy(id)
        };
        let stopped = match still() {
            true => self.stop_old_claude(&node).await,
            false => Ok(()),
        };
        if let Err(e) = &stopped {
            eprintln!("loom: 노드 {}를 재우지 못했다: {e}", id.0);
        }
        // 정리를 기다리는 동안 사람이 입력해 다시 일할 수 있다.
        if stopped.is_err() || !still() {
            self.sleeps.cancel(id);
            self.graph.wake_mail();
            return;
        }
        eprintln!("loom: 쉬는 완료 노드 {}를 재운다", id.0);
        let snapshot = Snapshot::now();
        snapshot.kill_trees(self.background_shells(&snapshot, &node));
        self.idle.end(id);
        self.host.stop(id);
        tokio::time::sleep(EXIT_WAIT).await;
        if self.sleeps.cancel(id) {
            eprintln!(
                "loom: 노드 {}의 프로세스가 끝나지 않았다. 재우지 않는다",
                id.0
            );
            self.graph.wake_mail();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Kind, Launched, NewChild, NodeState, Place, Report, TreeLimits};
    use crate::profile::AgentId;
    use crate::worktree::RepoId;

    fn launched(token: &str) -> Launched {
        Launched {
            token: token.into(),
            server: None,
            account: None,
            rules: None,
        }
    }

    /// 쉬는 대기가 이미 기다리는 동안 재우기가 시작되면, 그 대기는 메시지를 꺼내지 않고 노드는 `done`으로 남는다.
    /// 재우기를 멈추면 같은 대기가 그 메시지를 받는다(관문 검토 노드수명 P1).
    #[tokio::test]
    async fn an_open_idle_wait_takes_no_mail_while_the_node_is_put_to_sleep() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("loom.db");
        let database = crate::db::open(&db_path).unwrap();
        database
            .lock()
            .execute("INSERT INTO repos (id, path) VALUES (1, '/repo')", [])
            .unwrap();
        let app = Arc::new(
            App::new(
                database,
                &db_path,
                "127.0.0.1:0".parse().unwrap(),
                None,
                None,
            )
            .unwrap(),
        );
        let place = Place {
            repo: RepoId(1),
            worktree: "/repo".into(),
        };
        let claude = || Some(AgentId("claude".into()));
        let lead = app
            .graph
            .add_root(&place, claude(), None, &launched("lead"), |_| Ok(()))
            .unwrap();
        let child = NewChild {
            name: "worker",
            worktree: None,
            agent: claude(),
            brief: "",
            role: None,
            task: "t",
        };
        let limits = TreeLimits {
            max_depth: None,
            max_children: None,
        };
        let worker = app
            .graph
            .add_child(lead, child, limits, &launched("worker"), |_| Ok(()))
            .unwrap();
        let report = Report {
            summary: "finished",
            file: None,
            warning: None,
        };
        app.graph
            .done(worker, &report, None, &[], None, None)
            .unwrap();

        let waiting = app.clone();
        let mut idle =
            tokio::spawn(async move { waiting.next_mail(worker, Kind::ALL, usize::MAX).await });
        tokio::task::yield_now().await;
        assert!(app.sleeps.begin(worker));
        app.graph
            .note(lead, "worker", Kind::Queue, "new task", None)
            .unwrap();
        let held = tokio::time::timeout(Duration::from_millis(300), &mut idle).await;
        assert!(held.is_err(), "the idle wait took mail while stopping");
        assert!(matches!(
            app.graph.relations(worker).unwrap().me.state,
            NodeState::Done { .. }
        ));

        assert!(app.sleeps.cancel(worker));
        app.graph.wake_mail();
        let taken = tokio::time::timeout(Duration::from_secs(5), idle)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(taken[0].body, "new task");
    }

    #[test]
    fn a_node_is_due_after_resting_through_the_grace_time() {
        let sleeps = Sleeps::default();
        let (a, b) = (NodeId(1), NodeId(2));
        let t0 = Instant::now();
        assert!(sleeps.due(&[a, b], t0).is_empty());
        assert!(sleeps.due(&[a, b], t0 + DONE_GRACE / 2).is_empty());
        // b가 한 번 쉬지 않았다. 다시 잰다.
        assert!(sleeps.due(&[a], t0 + DONE_GRACE / 2).is_empty());
        let later = t0 + DONE_GRACE / 2 + Duration::from_secs(1);
        assert!(sleeps.due(&[a, b], later).is_empty());
        assert_eq!(sleeps.due(&[a, b], t0 + DONE_GRACE), vec![a]);
        assert_eq!(sleeps.due(&[a, b], later + DONE_GRACE), vec![a, b]);
    }
}
