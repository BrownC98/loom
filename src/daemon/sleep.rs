use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::Notify;

use super::App;
use super::relaunch::{EXIT_WAIT, Plan, profile_of};
use super::turn_end::IDLE_GRACE;
use crate::graph::{Active, Entry, IdleEnd, NodeId};
use crate::process_tree::Snapshot;
use crate::profile::{self, Profile, claude};
use crate::protocol::{MessageId, ServerLink};
use crate::screen::Screen;

const DONE_GRACE: Duration = Duration::from_secs(600);
const SWEEP: Duration = Duration::from_secs(30);
const WAKE_SUBMIT_AFTER: Duration = Duration::from_millis(500);

#[derive(Default)]
pub struct Sleeps {
    resting: Mutex<HashMap<NodeId, Instant>>,
    stopping: Mutex<HashSet<NodeId>>,
    knocked: Mutex<HashSet<NodeId>>,
    knock: Notify,
    unheard: Mutex<HashMap<NodeId, Unheard>>,
}

struct Unheard {
    oldest: MessageId,
    step: Step,
}

#[derive(Clone, Copy)]
enum Step {
    Seen(Instant),
    Typed(Instant),
    Relaunched,
}

#[derive(Clone, Copy)]
enum Wake {
    Type,
    Relaunch,
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

    fn unheard_due(&self, mail: &[(NodeId, MessageId)], now: Instant) -> Vec<(NodeId, Wake)> {
        let mut unheard = self.unheard.lock().unwrap();
        unheard.retain(|id, u| mail.contains(&(*id, u.oldest)));
        mail.iter()
            .filter_map(|&(id, oldest)| {
                let step = Step::Seen(now);
                match unheard.entry(id).or_insert(Unheard { oldest, step }).step {
                    Step::Seen(at) if now.duration_since(at) >= IDLE_GRACE => {
                        Some((id, Wake::Type))
                    }
                    Step::Typed(at) if now.duration_since(at) >= IDLE_GRACE => {
                        Some((id, Wake::Relaunch))
                    }
                    _ => None,
                }
            })
            .collect()
    }

    fn woke(&self, node: NodeId, wake: Wake, now: Instant) {
        if let Some(u) = self.unheard.lock().unwrap().get_mut(&node) {
            u.step = match wake {
                Wake::Type => Step::Typed(now),
                Wake::Relaunch => Step::Relaunched,
            };
        }
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
                if let Err(e) = app.wake_unheard(Instant::now()) {
                    eprintln!("loom: 쉬는 대기가 없는 노드를 보지 못했다: {e}");
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

    fn wake_unheard(self: &Arc<Self>, now: Instant) -> rusqlite::Result<()> {
        let due = self.sleeps.unheard_due(&self.graph.awake_with_mail()?, now);
        if due.is_empty() {
            return Ok(());
        }
        let mut active = self.graph.active()?;
        for (id, wake) in due {
            let Some(at) = active.iter().position(|n| n.id == id) else {
                continue;
            };
            let node = active.swap_remove(at);
            let Some(since) = self.unheard_since(&node)? else {
                continue;
            };
            if !self.untouched(id, since, &self.host.screen(id)) {
                continue;
            }
            self.sleeps.woke(id, wake, now);
            match wake {
                Wake::Type => {
                    if let Err(e) = self
                        .graph
                        .record_about(id, |node| Entry::NoIdleWait { node })
                    {
                        eprintln!("loom: 노드 {}의 빠진 쉬는 대기를 남기지 못했다: {e}", id.0);
                    }
                    let typed = self.host.write_if(id, |keys| {
                        let screen = keys.screen();
                        (self.untouched(id, since, &screen)
                            && claude::input(&screen.text) == Some(""))
                        .then(|| claude::wake_line().into_bytes())
                        .ok_or(())
                    });
                    if typed.is_ok() {
                        let app = self.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(WAKE_SUBMIT_AFTER).await;
                            let line = claude::wake_line();
                            let _ = app.host.write_if(id, |keys| {
                                (claude::input(&keys.screen().text) == Some(line.as_str()))
                                    .then(|| b"\r".to_vec())
                                    .ok_or(())
                            });
                        });
                    }
                    eprintln!(
                        "loom: 쉬는 대기가 없는 노드 {}{}",
                        id.0,
                        match typed {
                            Ok(()) => "에 알림 줄을 쳤다",
                            Err(()) => "의 입력칸이 비지 않아 알림 줄을 치지 않았다",
                        }
                    );
                }
                Wake::Relaunch => {
                    eprintln!("loom: 알림 줄로 깨지 않은 노드 {}를 다시 띄운다", id.0);
                    self.idle.end(id, IdleEnd::Relaunch);
                    let app = self.clone();
                    tokio::spawn(async move {
                        if let Err(e) = app.relaunch_or_queue(node).await {
                            eprintln!(
                                "loom: 쉬는 대기가 없는 노드 {}를 다시 띄우지 못했다: {e}",
                                id.0
                            );
                        }
                    });
                }
            }
        }
        Ok(())
    }

    fn unheard_since(&self, node: &Active) -> rusqlite::Result<Option<SystemTime>> {
        if profile_of(node) != Some(Profile::Claude)
            || self.idle.waiting(node.id)
            || self.relaunches.holds_mail(node.id)
            || self.sleeps.stopping(node.id)
        {
            return Ok(None);
        }
        let Some(transcript) = &node.transcript else {
            return Ok(None);
        };
        let place = self.graph.place_of(node.id)?;
        let path = crate::wsl::from_node(PathBuf::from(transcript), Path::new(&place.worktree));
        let changed = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        Ok(changed.filter(|at| {
            at.elapsed().is_ok_and(|age| age >= IDLE_GRACE) && claude::turn_ended(&path)
        }))
    }

    fn untouched(&self, node: NodeId, since: SystemTime, screen: &Screen) -> bool {
        !self.idle.typed_since(node, since)
            && profile::prompt(Some(Profile::Claude), screen).is_none()
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
            Some(Profile::Claude | Profile::Cursor | Profile::Pi) => {
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
        self.idle.end(id, IdleEnd::Sleep);
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
    use crate::daemon::nodes::human_typed;
    use crate::graph::{Launched, NewChild, NodeState, Place, Report};
    use crate::protocol::{AgentId, Kind, TreeLimits};
    use crate::worktree::RepoId;

    fn launched(token: &str) -> Launched {
        Launched {
            token: token.into(),
            server: None,
            account: None,
            rules: None,
        }
    }

    struct Fixture {
        dir: tempfile::TempDir,
        app: Arc<App>,
        lead: NodeId,
    }

    fn fixture() -> Fixture {
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
        let lead = app
            .graph
            .add_root(
                &place,
                Some(AgentId("claude".into())),
                None,
                &launched("lead"),
                |_| Ok(()),
            )
            .unwrap();
        Fixture { dir, app, lead }
    }

    fn add_worker(t: &Fixture, name: &str) -> NodeId {
        let child = NewChild {
            name,
            worktree: None,
            branched_from: None,
            agent: Some(AgentId("claude".into())),
            brief: "",
            role: None,
            task: "t",
            review: None,
        };
        let limits = TreeLimits {
            max_depth: None,
            max_children: None,
        };
        t.app
            .graph
            .add_child(t.lead, child, limits, &launched(name), |_| Ok(()))
            .unwrap()
    }

    #[tokio::test]
    async fn an_open_idle_wait_takes_no_mail_while_the_node_is_put_to_sleep() {
        let t = fixture();
        let (app, lead) = (t.app.clone(), t.lead);
        let worker = add_worker(&t, "worker");
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

    const ENDED: &str = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"끝\"}]}}\n{\"type\":\"system\",\"subtype\":\"stop_hook_summary\"}\n{\"type\":\"system\",\"subtype\":\"turn_duration\"}\n{\"type\":\"last-prompt\"}\n";
    const IN_TOOL: &str = "{\"type\":\"user\",\"message\":{\"content\":\"go\"}}\n{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Bash\"}]}}\n";

    fn unheard(t: &Fixture, name: &str, transcript: &str) -> NodeId {
        let node = add_worker(t, name);
        let path = t.dir.path().join(format!("{name}.jsonl"));
        std::fs::write(&path, transcript).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::now() - IDLE_GRACE * 2)
            .unwrap();
        t.app
            .graph
            .set_transcript(node, path.to_str().unwrap())
            .unwrap();
        t.app.idle.turn_started(node, "UserPromptSubmit".into());
        t.app
            .graph
            .note(t.lead, name, Kind::Queue, "new task", None)
            .unwrap();
        node
    }

    fn missed(t: &Fixture) -> Vec<(NodeId, bool)> {
        let mut found: Vec<(NodeId, bool)> = t
            .app
            .graph
            .recent_events()
            .unwrap()
            .into_iter()
            .filter_map(|r| match r.entry {
                Entry::NoIdleWait { node } => Some((node.id?, false)),
                Entry::Relaunched { node, .. } => Some((node.id?, true)),
                _ => None,
            })
            .collect();
        found.reverse();
        found
    }

    async fn settle(t: &Fixture, count: usize) -> Vec<(NodeId, bool)> {
        for _ in 0..500 {
            if missed(t).len() >= count {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        missed(t)
    }

    #[tokio::test]
    async fn an_idle_node_without_a_live_idle_wait_is_woken_then_relaunched() {
        let t = fixture();
        let deaf = unheard(&t, "deaf", ENDED);
        let busy = unheard(&t, "busy", IN_TOOL);
        let heard = unheard(&t, "heard", ENDED);
        let typing = unheard(&t, "typing", ENDED);
        let _wait = t.app.idle.open(heard);
        human_typed(&t.app, typing);
        let closed = unheard(&t, "closed", ENDED);
        drop(t.app.idle.open(closed));

        let t0 = Instant::now();
        t.app.wake_unheard(t0).unwrap();
        assert_eq!(missed(&t), [], "the mail is new");
        t.app.wake_unheard(t0 + IDLE_GRACE).unwrap();
        assert_eq!(missed(&t), [(deaf, false), (closed, false)]);
        assert!(t.app.busy(deaf), "a typed wake keeps the turn state");
        t.app.wake_unheard(t0 + IDLE_GRACE * 3 / 2).unwrap();
        assert_eq!(missed(&t).len(), 2, "the typed wake gets its grace");
        t.app.wake_unheard(t0 + IDLE_GRACE * 2).unwrap();
        for name in ["deaf", "closed"] {
            std::fs::remove_file(t.dir.path().join(format!("{name}.jsonl"))).unwrap();
        }
        let found = settle(&t, 4).await;
        assert_eq!(
            found,
            [(deaf, false), (closed, false), (deaf, true), (closed, true)]
        );
        t.app.wake_unheard(t0 + IDLE_GRACE * 10).unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(missed(&t), found, "once per oldest message");
        assert!(!found.iter().any(|(n, _)| [busy, heard, typing].contains(n)));
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
