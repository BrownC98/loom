use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::Notify;

use super::App;
use super::relaunch::{EXIT_WAIT, Guard, Plan, profile_of};
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

struct Seen {
    path: PathBuf,
    changed: SystemTime,
    typed_after: SystemTime,
    listening: bool,
}

struct Unheard {
    oldest: MessageId,
    step: Step,
    noted: bool,
}

#[derive(Clone, Copy)]
enum Step {
    Seen(Instant),
    Tried(Instant),
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
                let fresh = Unheard {
                    oldest,
                    step: Step::Seen(now),
                    noted: false,
                };
                match unheard.entry(id).or_insert(fresh).step {
                    Step::Seen(at) if now.duration_since(at) >= IDLE_GRACE => {
                        Some((id, Wake::Type))
                    }
                    Step::Tried(at) if now.duration_since(at) >= IDLE_GRACE => {
                        Some((id, Wake::Relaunch))
                    }
                    _ => None,
                }
            })
            .collect()
    }

    fn first_miss(&self, node: NodeId) -> bool {
        self.unheard
            .lock()
            .unwrap()
            .get_mut(&node)
            .is_some_and(|u| !std::mem::replace(&mut u.noted, true))
    }

    fn woke(&self, node: NodeId, wake: Wake, now: Instant) {
        if let Some(u) = self.unheard.lock().unwrap().get_mut(&node) {
            u.step = match wake {
                Wake::Type => Step::Tried(now),
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
            let queued = self.relaunches.at_turn_end(id);
            if self.relaunches.holds_mail(id) && !queued {
                continue;
            }
            let Some(seen) = self.unheard_since(&node)? else {
                continue;
            };
            if seen.listening && !queued {
                continue;
            }
            let since = seen.typed_after;
            if self.sleeps.first_miss(id) {
                eprintln!("loom: 노드 {}의 쉬는 대기가 없다", id.0);
                if let Err(e) = self
                    .graph
                    .record_about(id, |node| Entry::NoIdleWait { node })
                {
                    eprintln!("loom: 노드 {}의 빠진 쉬는 대기를 남기지 못했다: {e}", id.0);
                }
            }
            if queued || matches!(wake, Wake::Relaunch) {
                let guard = self.restart_guard(id, seen);
                if !guard(&self.host.screen(id)) {
                    continue;
                }
                if queued {
                    eprintln!("loom: 노드 {}의 턴 끝 다시 띄우기를 지금 한다", id.0);
                    self.sleeps.woke(id, Wake::Type, now);
                    self.run_at_turn_end(id, Some(guard));
                    continue;
                }
                self.sleeps.woke(id, Wake::Relaunch, now);
                eprintln!("loom: 알림 줄로 깨지 않은 노드 {}를 다시 띄운다", id.0);
                let app = self.clone();
                tokio::spawn(async move {
                    if let Err(e) = app.relaunch_unless_used(node, guard).await {
                        eprintln!(
                            "loom: 쉬는 대기가 없는 노드 {}를 다시 띄우지 못했다: {e}",
                            id.0
                        );
                    }
                });
                continue;
            }
            let typed = self.host.write_if(id, |keys| {
                let screen = keys.screen();
                (self.untouched(id, since, &screen) && claude::input(&screen.text) == Some(""))
                    .then(|| claude::wake_line().into_bytes())
                    .ok_or(())
            });
            if typed.is_err() {
                continue;
            }
            eprintln!("loom: 쉬는 대기가 없는 노드 {}에 알림 줄을 쳤다", id.0);
            self.sleeps.woke(id, Wake::Type, now);
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
        Ok(())
    }

    fn restart_guard(self: &Arc<Self>, node: NodeId, seen: Seen) -> Guard {
        let app = self.clone();
        let line = claude::wake_line();
        Arc::new(move |screen: &Screen| {
            let input = claude::input(&screen.text);
            std::fs::metadata(&seen.path)
                .and_then(|m| m.modified())
                .is_ok_and(|at| at == seen.changed)
                && app.idle.waiting(node) == seen.listening
                && app.untouched(node, seen.typed_after, screen)
                && (input == Some("") || input == Some(line.as_str()))
        })
    }

    fn unheard_since(&self, node: &Active) -> rusqlite::Result<Option<Seen>> {
        if profile_of(node) != Some(Profile::Claude) || self.sleeps.stopping(node.id) {
            return Ok(None);
        }
        let Some(transcript) = &node.transcript else {
            return Ok(None);
        };
        let place = self.graph.place_of(node.id)?;
        let path = crate::wsl::from_node(PathBuf::from(transcript), Path::new(&place.worktree));
        let Ok(changed) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
            return Ok(None);
        };
        if self.idle.waiting(node.id) {
            return Ok(Some(Seen {
                path,
                changed,
                typed_after: SystemTime::now(),
                listening: true,
            }));
        }
        let ended =
            changed.elapsed().is_ok_and(|age| age >= IDLE_GRACE) && claude::turn_ended(&path);
        Ok(ended.then_some(Seen {
            path,
            changed,
            typed_after: changed,
            listening: false,
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
            Some(Profile::Claude | Profile::Cursor | Profile::Pi | Profile::Omp) => {
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
    #[cfg(unix)]
    use crate::daemon::nodes::human_typed;
    #[cfg(unix)]
    use crate::daemon::relaunch::{Relaunched, Stop};
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
                crate::tunnel::Door::new().0,
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
            keep: &[],
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
    #[cfg(unix)]
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

    #[cfg(unix)]
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
    async fn a_node_without_a_readable_input_box_is_noted_once_and_kept() {
        let t = fixture();
        let blind = unheard(&t, "blind", ENDED);
        let t0 = Instant::now();
        for step in 0..6 {
            t.app.wake_unheard(t0 + IDLE_GRACE * step).unwrap();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(missed(&t), [(blind, false)]);
        assert!(t.app.busy(blind));
    }

    #[cfg(unix)]
    fn boxed(input: &str) -> String {
        format!(
            "● 끝났습니다.\r\n\r\n────────\r\n{input}\r\n────────\r\n  ⏵⏵ bypass permissions on\r\n"
        )
    }

    #[cfg(unix)]
    fn screen(t: &Fixture, node: NodeId, input: &str) {
        let mut cmd = portable_pty::CommandBuilder::new("sh");
        cmd.args([
            "-c",
            "stty -icanon -echo; printf '%s' \"$SCREEN\"; exec cat >\"$TYPED\"",
        ]);
        cmd.env("SCREEN", boxed(input));
        cmd.env("TYPED", t.dir.path().join(format!("typed-{}", node.0)));
        host_pty(t, node, cmd);
    }

    #[cfg(unix)]
    fn redrawn(t: &Fixture, node: NodeId, first: &str, then: &str) {
        let mut cmd = portable_pty::CommandBuilder::new("sh");
        cmd.args([
            "-c",
            "stty -icanon -echo; printf '%s' \"$FIRST\"; head -c 1 >/dev/null; printf '\\033[2J\\033[H%s' \"$THEN\"; exec cat >/dev/null",
        ]);
        cmd.env("FIRST", boxed(first));
        cmd.env("THEN", boxed(then));
        host_pty(t, node, cmd);
    }

    #[cfg(unix)]
    fn host_pty(t: &Fixture, node: NodeId, cmd: portable_pty::CommandBuilder) {
        let hosts = t.dir.path().join("pty").join("nodes");
        crate::node_host::in_process(&hosts, node, cmd);
        let app = t.app.clone();
        let exited = t.dir.path().join(format!("exited-{}", node.0));
        t.app
            .host
            .resume(node, move |_| {
                std::fs::write(exited, "").unwrap();
                app.relaunches.exited(node);
            })
            .unwrap();
        while claude::input(&t.app.host.screen(node).text).is_none() {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn use_of_the_old_node_after_the_wake_decision_keeps_its_process() {
        let t = fixture();
        let drafted = unheard(&t, "drafted", ENDED);
        let typed = unheard(&t, "typed", ENDED);
        let spoke = unheard(&t, "spoke", ENDED);
        let quiet = unheard(&t, "quiet", ENDED);
        redrawn(&t, drafted, "❯ ", "❯ 고쳐 줘");
        for node in [typed, spoke, quiet] {
            screen(&t, node, "❯ ");
        }
        let active = t.app.graph.active().unwrap();
        let decided: Vec<(Active, Guard)> = active
            .into_iter()
            .filter(|n| [drafted, typed, spoke, quiet].contains(&n.id))
            .map(|n| {
                let seen = t.app.unheard_since(&n).unwrap().unwrap();
                let guard = t.app.restart_guard(n.id, seen);
                assert!(
                    guard(&t.app.host.screen(n.id)),
                    "the sweep decides to restart"
                );
                (n, guard)
            })
            .collect();

        t.app
            .host
            .write_if(drafted, |_| {
                Ok::<_, std::convert::Infallible>("고".as_bytes().to_vec())
            })
            .unwrap();
        while claude::input(&t.app.host.screen(drafted).text) != Some("고쳐 줘") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        human_typed(&t.app, typed);
        let log = t.dir.path().join("spoke.jsonl");
        std::fs::write(&log, format!("{ENDED}{ENDED}")).unwrap();

        for (node, guard) in decided {
            let id = node.id;
            let discarded = std::sync::atomic::AtomicBool::new(false);
            let discard = || discarded.store(true, std::sync::atomic::Ordering::SeqCst);
            let stop = t.app.end_old(&node, discard, Some(guard)).await;
            let kept = discarded.load(std::sync::atomic::Ordering::SeqCst);
            if id == quiet {
                assert_eq!((stop, kept), (Ok(Stop::Stopped), false));
                continue;
            }
            assert_eq!((stop, kept), (Ok(Stop::Kept), true), "node {}", id.0);
            assert!(t.app.busy(id), "node {} keeps its turn state", id.0);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        let exited = |node: NodeId| t.dir.path().join(format!("exited-{}", node.0)).exists();
        assert_eq!(
            [drafted, typed, spoke, quiet].map(exited),
            [false, false, false, true],
            "only the unused node stops"
        );
        assert_eq!(
            claude::input(&t.app.host.screen(drafted).text),
            Some("고쳐 줘")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_idle_node_without_a_live_idle_wait_is_woken_then_relaunched() {
        let t = fixture();
        let deaf = unheard(&t, "deaf", ENDED);
        let busy = unheard(&t, "busy", IN_TOOL);
        let heard = unheard(&t, "heard", ENDED);
        let typing = unheard(&t, "typing", ENDED);
        let closed = unheard(&t, "closed", ENDED);
        let draft = unheard(&t, "draft", ENDED);
        let queued = unheard(&t, "queued", ENDED);
        for node in [deaf, busy, heard, typing, closed, queued] {
            screen(&t, node, "❯ ");
        }
        screen(&t, draft, "❯ 고쳐 줘");
        let _wait = t.app.idle.open(heard);
        human_typed(&t.app, typing);
        drop(t.app.idle.open(closed));
        t.app
            .idle
            .typed_at(draft, SystemTime::now() - IDLE_GRACE * 3);
        let active = t.app.graph.active().unwrap();
        let held = active.into_iter().find(|n| n.id == queued).unwrap();
        assert_eq!(
            t.app.relaunch_or_queue(held).await,
            Ok(Relaunched::AtTurnEnd)
        );
        let gone = |names: &[&str]| {
            for name in names {
                std::fs::remove_file(t.dir.path().join(format!("{name}.jsonl"))).unwrap();
            }
        };

        let t0 = Instant::now();
        t.app.wake_unheard(t0).unwrap();
        assert_eq!(missed(&t), [], "the mail is new");
        t.app.wake_unheard(t0 + IDLE_GRACE).unwrap();
        gone(&["queued"]);
        let noticed = [
            (deaf, false),
            (typing, false),
            (closed, false),
            (draft, false),
            (queued, false),
            (queued, true),
        ];
        assert_eq!(settle(&t, 6).await, noticed);
        assert!(t.app.busy(deaf), "a typed wake keeps the turn state");
        let typed =
            || std::fs::read_to_string(t.dir.path().join(format!("typed-{}", deaf.0))).unwrap();
        while typed().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(typed(), claude::wake_line());
        t.app.wake_unheard(t0 + IDLE_GRACE * 3 / 2).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(missed(&t), noticed, "the typed wake gets its grace");
        t.app.wake_unheard(t0 + IDLE_GRACE * 2).unwrap();
        gone(&["deaf", "closed"]);
        let found = settle(&t, 8).await;
        assert_eq!(found[6..], [(deaf, true), (closed, true)]);
        t.app.wake_unheard(t0 + IDLE_GRACE * 10).unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            missed(&t),
            found,
            "once per oldest message, never over a draft"
        );
        assert!(!found.iter().any(|(n, _)| [busy, heard].contains(n)));
        for node in [typing, draft] {
            let file = t.dir.path().join(format!("typed-{}", node.0));
            assert_eq!(
                std::fs::read_to_string(file).unwrap(),
                "",
                "nothing typed over a draft"
            );
        }
    }

    fn idle_request(
        t: &Fixture,
        token: &str,
    ) -> tokio::task::JoinHandle<
        Result<axum::Json<Vec<crate::protocol::Delivery>>, crate::daemon::AgentError>,
    > {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        tokio::spawn(crate::daemon::commands::agent_idle(
            axum::extract::State(t.app.clone()),
            headers,
            axum::Json(None),
        ))
    }

    async fn taken(
        wait: tokio::task::JoinHandle<
            Result<axum::Json<Vec<crate::protocol::Delivery>>, crate::daemon::AgentError>,
        >,
    ) -> String {
        let axum::Json(taken) = tokio::time::timeout(Duration::from_secs(5), wait)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        taken[0].body.clone()
    }

    #[tokio::test]
    async fn a_cancelled_wake_restart_frees_an_idle_wait_that_came_during_preparation() {
        let t = fixture();
        let fallback = add_worker(&t, "fallback");
        t.app.relaunches.hold(fallback);
        let wait = idle_request(&t, "fallback");
        tokio::time::sleep(Duration::from_millis(50)).await;
        t.app
            .graph
            .note(t.lead, "fallback", Kind::Queue, "for fallback", None)
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !wait.is_finished(),
            "the wait holds while the restart prepares"
        );
        t.app.kept_running(fallback);
        assert_eq!(taken(wait).await, "for fallback");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_kept_queued_plan_runs_only_when_the_guard_passes_later() {
        let t = fixture();
        let queued = unheard(&t, "queued", ENDED);
        redrawn(&t, queued, "❯ 고쳐 줘", "❯ ");
        let held = t
            .app
            .graph
            .active()
            .unwrap()
            .into_iter()
            .find(|n| n.id == queued)
            .unwrap();
        assert_eq!(
            t.app.relaunch_or_queue(held).await,
            Ok(Relaunched::AtTurnEnd)
        );
        t.app.relaunches.hold(queued);
        let wait = idle_request(&t, "queued");
        tokio::time::sleep(Duration::from_millis(50)).await;
        human_typed(&t.app, queued);
        t.app.kept_at_turn_end(queued);
        tokio::time::sleep(Duration::from_millis(200)).await;
        let relaunched = || missed(&t).contains(&(queued, true));
        let exited = || t.dir.path().join(format!("exited-{}", queued.0)).exists();
        assert!(!wait.is_finished() && !relaunched() && !exited());
        assert!(
            t.app.relaunches.at_turn_end(queued),
            "the plan stays queued"
        );

        let t0 = Instant::now();
        t.app.wake_unheard(t0).unwrap();
        t.app.wake_unheard(t0 + IDLE_GRACE).unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!relaunched() && !exited(), "no restart over the draft");
        assert_eq!(
            claude::input(&t.app.host.screen(queued).text),
            Some("고쳐 줘")
        );

        t.app
            .host
            .write_if(queued, |_| {
                human_typed(&t.app, queued);
                Ok::<_, std::convert::Infallible>(b"\x7f".to_vec())
            })
            .unwrap();
        while claude::input(&t.app.host.screen(queued).text) != Some("") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        t.app.wake_unheard(t0 + IDLE_GRACE * 2).unwrap();
        std::fs::remove_file(t.dir.path().join("queued.jsonl")).unwrap();
        assert_eq!(taken(wait).await, "new task");
        assert!(relaunched(), "the queued plan ran once the box was empty");
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_restarted_daemon_shows_the_last_screen_of_a_sleeping_node() {
        use futures_util::StreamExt;
        use tokio_tungstenite::tungstenite::Message;

        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("loom.db");
        let database = crate::db::open(&db_path).unwrap();
        database
            .lock()
            .execute("INSERT INTO repos (id, path) VALUES (1, '/repo')", [])
            .unwrap();
        let start = |database| {
            Arc::new(
                App::new(
                    database,
                    &db_path,
                    "127.0.0.1:0".parse().unwrap(),
                    None,
                    None,
                    crate::tunnel::Door::new().0,
                )
                .unwrap(),
            )
        };
        let app = start(database);
        let place = Place {
            repo: RepoId(1),
            worktree: "/repo".into(),
        };
        let lead = app
            .graph
            .add_root(&place, None, None, &launched("lead"), |_| Ok(()))
            .unwrap();
        let child = NewChild {
            name: "worker",
            worktree: None,
            branched_from: None,
            agent: None,
            brief: "",
            role: None,
            task: "t",
            review: None,
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
            keep: &[],
        };
        app.graph
            .done(worker, &report, None, &[], None, None)
            .unwrap();
        let mut cmd = portable_pty::CommandBuilder::new("sh");
        cmd.args(["-c", "printf 'last screen'; read line"]);
        crate::node_host::in_process(&dir.path().join("pty/nodes"), worker.0, cmd);
        app.resume().await.unwrap();
        app.host.resize(worker, 77, 22);
        while !app.host.tail(worker).contains("last screen") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let node = app
            .graph
            .active()
            .unwrap()
            .into_iter()
            .find(|n| n.id == worker)
            .unwrap();
        assert!(app.sleeps.begin(worker));
        let sleeping = app.clone();
        tokio::spawn(async move { sleeping.put_to_sleep(node).await });
        while app.host.kept(worker).is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(app.graph.asleep(worker).unwrap());

        let restarted = start(crate::db::open(&db_path).unwrap());
        restarted.resume().await.unwrap();
        assert!(restarted.host.attach(worker).is_none());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let routes = super::super::nodes::routes().with_state(restarted.clone());
        tokio::spawn(async move { axum::serve(listener, routes).await });
        let url = format!("ws://{address}/api/nodes/{}/pty", worker.0);
        let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        let mut next = async || {
            tokio::time::timeout(Duration::from_secs(5), socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap()
        };
        assert_eq!(
            next().await,
            Message::text(r#"{"size":{"cols":77,"rows":22}}"#)
        );
        assert_eq!(next().await, Message::text(r#""settled""#));
        let Message::Binary(screen) = next().await else {
            panic!("the saved screen did not come");
        };
        assert!(String::from_utf8_lossy(&screen).ends_with("last screen"));

        restarted.remove(worker).unwrap();
        assert!(restarted.host.kept(worker).is_none());
        assert!(matches!(next().await, Message::Close(_)));
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
