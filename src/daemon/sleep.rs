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

const DONE_GRACE: Duration = Duration::from_secs(600);
const SWEEP: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct Sleeps {
    resting: Mutex<HashMap<NodeId, Instant>>,
    stopping: Mutex<HashSet<NodeId>>,
    knocked: Mutex<HashSet<NodeId>>,
    knock: Notify,
}

impl Sleeps {
    pub fn stopping(&self, node: NodeId) -> bool {
        self.stopping.lock().unwrap().contains(&node)
    }

    pub fn unless_stopping<T>(&self, node: NodeId, take: impl FnOnce() -> T) -> Option<T> {
        let stopping = self.stopping.lock().unwrap();
        (!stopping.contains(&node)).then(take)
    }

    fn begin(&self, node: NodeId) -> bool {
        self.stopping.lock().unwrap().insert(node)
    }

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
            Some(Profile::OpenCode2) | None => false,
        }
    }

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
        assert!(sleeps.due(&[a], t0 + DONE_GRACE / 2).is_empty());
        let later = t0 + DONE_GRACE / 2 + Duration::from_secs(1);
        assert!(sleeps.due(&[a, b], later).is_empty());
        assert_eq!(sleeps.due(&[a, b], t0 + DONE_GRACE), vec![a]);
        assert_eq!(sleeps.due(&[a, b], later + DONE_GRACE), vec![a, b]);
    }
}
