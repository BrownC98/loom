use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::Notify;
use tokio::sync::broadcast::error::RecvError;

use super::App;
use super::relaunch::{EXIT_WAIT, Guard, Stop, profile_of};
use crate::graph::{
    Active, AgentChange, AgentEvent, AgentState, Entry, IdleEnd, NodeId, Phase, Plan,
};
use crate::process_tree::Snapshot;
use crate::profile::{self, Profile, claude};
use crate::protocol::ServerLink;
use crate::screen::Screen;

const DONE_GRACE: Duration = Duration::from_secs(600);
const SWEEP: Duration = Duration::from_secs(30);
const STOP_HOOK_GRACE: Duration = Duration::from_secs(60);
const RUNNING_STATUS: &str = "esc to interrupt";

type Recovery = (Active, Plan, Guard, fn(&App, NodeId));

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
            let started = Instant::now();
            let mut next_sweep = started;
            loop {
                let mut mail = pin!(app.graph.mail_changed());
                mail.as_mut().enable();
                let mut knock = pin!(app.sleeps.knock.notified());
                knock.as_mut().enable();
                if let Err(e) = app.wake_sleepers() {
                    eprintln!("loom: 잠든 노드를 보지 못했다: {e}");
                }
                app.recover_held_with_mail();
                if let Err(e) = app.end_unhooked_turns() {
                    eprintln!("loom: stop 훅이 닿지 않은 노드를 보지 못했다: {e}");
                }
                if Instant::now() >= next_sweep {
                    if let Err(e) = app.sleep_resting() {
                        eprintln!("loom: 쉬는 노드를 보지 못했다: {e}");
                    }
                    if started.elapsed() >= SWEEP
                        && let Err(e) = app.hear_lost_waits()
                    {
                        eprintln!("loom: 쉬는 대기를 잃은 노드를 보지 못했다: {e}");
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

    pub(super) fn watch_agents(self: &Arc<Self>) {
        let Some(mut changes) = self.graph.agent_changes() else {
            return;
        };
        if let Err(e) = self.recover_stored() {
            eprintln!("loom: 듣지 못하는 노드를 보지 못했다: {e}");
        }
        let app = self.clone();
        tokio::spawn(async move {
            while let Some(change) = changes.recv().await {
                app.agent_changed(change);
            }
        });
    }

    fn agent_changed(self: &Arc<Self>, change: AgentChange) {
        match change.to {
            AgentState::Deaf => {}
            AgentState::Blocked { .. } => return,
            _ => return self.relaunches.heard(change.node),
        }
        let lost = match change.by {
            AgentEvent::WaitClosed {
                by: IdleEnd::Gone | IdleEnd::Error,
            } => change.from == AgentState::Listening,
            AgentEvent::ServerLost | AgentEvent::DialogCleared => true,
            AgentEvent::TurnEnded | AgentEvent::RelaunchFailed => false,
            _ => self.relaunches.held(change.node),
        };
        if lost {
            self.recover(change.node);
        }
    }

    fn recover_stored(self: &Arc<Self>) -> rusqlite::Result<()> {
        for node in self.graph.active()? {
            let id = node.id;
            if self.agent_state(id) != Some(AgentState::Deaf) {
                continue;
            }
            let Some(mut screen) = self.host.attach(id).filter(|a| a.backlog.is_empty()) else {
                self.recover(id);
                continue;
            };
            let app = self.clone();
            tokio::spawn(async move {
                if !matches!(screen.output.recv().await, Err(RecvError::Closed)) {
                    app.recover(id);
                }
            });
        }
        Ok(())
    }

    fn recover(self: &Arc<Self>, id: NodeId) {
        let (node, plan, guard, kept) = match self.recovering(id) {
            Ok(Some(recovery)) => recovery,
            Ok(None) => return,
            Err(e) => return eprintln!("loom: 듣지 못하는 노드 {}를 보지 못했다: {e}", id.0),
        };
        let app = self.clone();
        tokio::spawn(async move {
            match app.relaunch(node, plan, Some(guard)).await {
                Ok(Stop::Stopped) => {}
                Ok(Stop::Kept) => {
                    kept(&app, id);
                    app.refused(id);
                }
                Err(e) => {
                    eprintln!("loom: 듣지 못하는 노드 {}를 살리지 못했다: {e}", id.0);
                    app.host.stop(id);
                    if let Err(e) = app.graph.lost(id) {
                        eprintln!("loom: 노드 {}를 exited로 바꾸지 못했다: {e}", id.0);
                    }
                    app.graph.wake_mail();
                }
            }
        });
    }

    fn refused(&self, id: NodeId) {
        let newest = self.graph.newest_mail(id).ok().flatten();
        self.relaunches.refused(id, newest);
    }

    fn recover_held_with_mail(self: &Arc<Self>) {
        let mailed = self
            .relaunches
            .mail_since_refused(|id| self.graph.newest_mail(id).ok().flatten());
        for id in mailed {
            self.refused(id);
            self.recover(id);
        }
    }

    fn recovering(self: &Arc<Self>, id: NodeId) -> rusqlite::Result<Option<Recovery>> {
        if self.agent_state(id) != Some(AgentState::Deaf) || self.sleeps.stopping(id) {
            return Ok(None);
        }
        let Some(node) = self.graph.active()?.into_iter().find(|n| n.id == id) else {
            return Ok(None);
        };
        if profile_of(&node).is_none() {
            return Ok(None);
        }
        let since = self.relaunches.held(id).then(SystemTime::now);
        let guard = self.restart_guard(&node, since)?;
        if !guard(&self.host.screen(id)) {
            self.refused(id);
            eprintln!(
                "loom: 듣지 못하는 노드 {}를 사람이 쓰고 있어 다음 사건까지 기다린다",
                id.0
            );
            return Ok(None);
        }
        let claimed = self.graph.plan_relaunch(id, |now| match now.pending {
            _ if now.ended => (now.pending, None),
            None => {
                let kept: fn(&App, NodeId) = App::kept_running;
                (
                    Some((Phase::Running, Plan::Resume)),
                    Some((Plan::Resume, kept)),
                )
            }
            Some((Phase::AtTurnEnd, plan)) => {
                let kept: fn(&App, NodeId) = App::kept_at_turn_end;
                (Some((Phase::Running, plan.clone())), Some((plan, kept)))
            }
            running => (running, None),
        })?;
        let Some((plan, kept)) = claimed else {
            return Ok(None);
        };
        eprintln!(
            "loom: 듣지 못하는 노드 {}를 같은 세션으로 다시 띄운다",
            id.0
        );
        Ok(Some((node, plan, guard, kept)))
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

    fn hear_lost_waits(self: &Arc<Self>) -> rusqlite::Result<()> {
        for node in self.graph.active()? {
            let id = node.id;
            let hooked = profile_of(&node).is_some_and(Profile::hooked);
            if !hooked || self.sleeps.stopping(id) {
                continue;
            }
            self.idle.lose_unheld(id);
        }
        Ok(())
    }

    fn end_unhooked_turns(self: &Arc<Self>) -> rusqlite::Result<()> {
        for node in self.graph.active()? {
            let id = node.id;
            let profile = profile_of(&node);
            if !profile.is_some_and(Profile::ends_turns_by_stop_hook)
                || self.sleeps.stopping(id)
                || self.agent_state(id) != Some(AgentState::Turn)
            {
                continue;
            }
            let ended = match profile {
                Some(Profile::Claude) => self.claude_turn_ended(&node)?,
                _ => self.turn_ends.said_before(id, STOP_HOOK_GRACE),
            };
            if !ended {
                continue;
            }
            eprintln!("loom: 노드 {}의 턴이 끝났는데 stop 훅이 닿지 않았다", id.0);
            if let Err(e) = self
                .graph
                .record_about(id, |node| Entry::NoIdleWait { node })
            {
                eprintln!("loom: 노드 {}의 빠진 쉬는 대기를 남기지 못했다: {e}", id.0);
            }
            self.feed(id, AgentEvent::TurnEnded);
            self.recover(id);
        }
        Ok(())
    }

    fn claude_turn_ended(&self, node: &Active) -> rusqlite::Result<bool> {
        let Some(path) = self.transcript(node)? else {
            return Ok(false);
        };
        let Some(began) = self.graph.turn_began(node.id)? else {
            return Ok(false);
        };
        let settled = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .is_ok_and(|at| at > began && at.elapsed().is_ok_and(|age| age >= STOP_HOOK_GRACE));
        let ended = self.at_turn_end(node.id) || claude::turn_ended(&path);
        Ok(settled && ended && !turn_shown(&self.host.screen(node.id).text))
    }

    fn transcript(&self, node: &Active) -> rusqlite::Result<Option<PathBuf>> {
        let Some(transcript) = &node.transcript else {
            return Ok(None);
        };
        let place = self.graph.place_of(node.id)?;
        Ok(Some(crate::wsl::from_node(
            PathBuf::from(transcript),
            Path::new(&place.worktree),
        )))
    }

    fn restart_guard(
        self: &Arc<Self>,
        node: &Active,
        since: Option<SystemTime>,
    ) -> rusqlite::Result<Guard> {
        let (app, id, profile) = (self.clone(), node.id, profile_of(node));
        let path = self.transcript(node)?;
        let modified = |path: &Option<PathBuf>| {
            path.as_ref()
                .and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        };
        let changed = modified(&path);
        let typed_after = since.or(changed).unwrap_or_else(SystemTime::now);
        Ok(Arc::new(move |screen: &Screen| {
            let quiet = match profile {
                Some(Profile::Claude) => {
                    claude::input(&screen.text) == Some("") && !turn_shown(&screen.text)
                }
                _ => true,
            };
            quiet
                && modified(&path) == changed
                && app.agent_state(id) == Some(AgentState::Deaf)
                && !app.idle.typed_since(id, typed_after)
                && profile::prompt(profile, screen).is_none()
        }))
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
        if self.holds_mail(node.id) || !self.listening(node.id) {
            return false;
        }
        match profile_of(node) {
            Some(profile) if profile.hooked() => node.transcript.is_some(),
            Some(Profile::Codex) => matches!(
                node.server,
                Some(ServerLink::Codex {
                    thread: Some(_),
                    ..
                })
            ),
            Some(_) | None => false,
        }
    }

    fn listening(&self, node: NodeId) -> bool {
        self.agent_state(node) == Some(AgentState::Listening)
    }

    async fn put_to_sleep(&self, node: Active) {
        let id = node.id;
        let still = || {
            self.graph
                .done_children()
                .is_ok_and(|done| done.contains(&id))
                && self.listening(id)
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

fn turn_shown(screen: &str) -> bool {
    let lines: Vec<&str> = screen.lines().map(str::trim_end).collect();
    let rules: Vec<usize> = (0..lines.len())
        .filter(|&i| !lines[i].is_empty() && lines[i].chars().all(|c| c == '─'))
        .collect();
    let (above, footer) = match rules[..] {
        [.., top, bottom] => (&lines[..top], &lines[bottom + 1..]),
        _ => (&lines[..], &lines[..0]),
    };
    footer.iter().any(|line| line.contains(RUNNING_STATUS)) || above.iter().any(|l| spinner(l))
}

fn spinner(line: &str) -> bool {
    let mut chars = line.chars();
    let glyph = chars
        .next()
        .is_some_and(|c| !c.is_alphanumeric() && !c.is_whitespace() && !"❯●⎿─".contains(c));
    let rest = chars.as_str();
    glyph
        && rest.starts_with(' ')
        && rest[1..]
            .split(' ')
            .next()
            .is_some_and(|word| word.ends_with('…') && word.chars().count() > 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use crate::daemon::nodes::human_typed;
    use crate::daemon::relaunch::Relaunched;
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
                tokio::sync::mpsc::unbounded_channel().0,
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
        add_agent(t, name, "claude")
    }

    fn add_agent(t: &Fixture, name: &str, agent: &str) -> NodeId {
        let child = NewChild {
            name,
            worktree: None,
            branched_from: None,
            agent: Some(AgentId(agent.into())),
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

    #[tokio::test]
    async fn a_hook_take_hands_no_mail_to_a_held_or_stopping_node() {
        let t = fixture();
        let (app, lead) = (t.app.clone(), t.lead);
        let worker = add_worker(&t, "worker");
        app.graph
            .note(lead, "worker", Kind::Queue, "new task", None)
            .unwrap();
        let take = async || {
            let mut headers = axum::http::HeaderMap::new();
            headers.insert(
                axum::http::header::AUTHORIZATION,
                "Bearer worker".parse().unwrap(),
            );
            let req = crate::daemon::TakeRequest {
                hook: "PostToolUse".into(),
                kinds: Kind::ALL.to_vec(),
                limit_bytes: crate::profile::BATCH_LIMIT_BYTES,
                transcript: None,
                human_input: false,
                prompt: None,
                turn: None,
            };
            let reply = crate::daemon::commands::agent_take(
                axum::extract::State(app.clone()),
                headers,
                axum::Json(req),
            )
            .await;
            let axum::Json(taken) = reply.unwrap_or_else(|(_, e)| panic!("{e}"));
            taken.messages
        };
        app.hold(worker);
        assert_eq!(take().await, [], "a pending relaunch holds the mail");
        app.kept_running(worker);
        assert!(app.sleeps.begin(worker));
        assert_eq!(take().await, [], "the node is put to sleep");
        assert!(app.sleeps.cancel(worker));
        let taken = take().await;
        assert_eq!(taken.len(), 1);
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
            .set_modified(SystemTime::now() - STOP_HOOK_GRACE * 2)
            .unwrap();
        t.app
            .graph
            .set_transcript(node, path.to_str().unwrap())
            .unwrap();
        t.app.idle.turn_started(node, "UserPromptSubmit".into());
        t.app.graph.backdate_agent_state(node, STOP_HOOK_GRACE * 3);
        t.app
            .graph
            .note(t.lead, name, Kind::Queue, "new task", None)
            .unwrap();
        node
    }

    #[tokio::test]
    async fn mail_that_wakes_a_node_after_its_last_turn_end_is_not_a_missed_stop_hook() {
        let t = fixture();
        let woken = add_worker(&t, "woken");
        let path = t.dir.path().join("woken.jsonl");
        std::fs::write(&path, ENDED).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::now() - STOP_HOOK_GRACE * 2)
            .unwrap();
        t.app
            .graph
            .set_transcript(woken, path.to_str().unwrap())
            .unwrap();
        let wait = t.app.idle.open(woken);
        t.app
            .graph
            .note(t.lead, "woken", Kind::Queue, "new task", None)
            .unwrap();
        wait.hand_mail();
        assert_eq!(t.app.agent_state(woken), Some(AgentState::Turn));
        t.app.end_unhooked_turns().unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(missed(&t), []);
        assert_eq!(t.app.agent_state(woken), Some(AgentState::Turn));
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
    async fn a_turn_end_without_a_stop_hook_is_noted_once_and_a_node_without_an_input_box_is_kept()
    {
        let t = fixture();
        let blind = unheard(&t, "blind", ENDED);
        for _ in 0..3 {
            t.app.end_unhooked_turns().unwrap();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(missed(&t), [(blind, false)]);
        assert_eq!(t.app.agent_state(blind), Some(AgentState::Deaf));
        assert!(t.app.relaunches.held(blind));
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
    async fn use_of_the_old_node_after_the_recovery_decision_keeps_its_process() {
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
                t.app.feed(n.id, AgentEvent::TurnEnded);
                let guard = t.app.restart_guard(&n, None).unwrap();
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
            assert_eq!(
                t.app.agent_state(id),
                Some(AgentState::Deaf),
                "node {} keeps its state",
                id.0
            );
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
    async fn a_turn_end_without_a_stop_hook_makes_the_node_deaf_and_relaunches_it_at_once() {
        let t = fixture();
        let deaf = unheard(&t, "deaf", ENDED);
        let busy = unheard(&t, "busy", IN_TOOL);
        let heard = unheard(&t, "heard", ENDED);
        let typing = unheard(&t, "typing", ENDED);
        let draft = unheard(&t, "draft", ENDED);
        let fresh = unheard(&t, "fresh", ENDED);
        let queued = unheard(&t, "queued", ENDED);
        for node in [deaf, busy, heard, typing, fresh, queued] {
            screen(&t, node, "❯ ");
        }
        screen(&t, draft, "❯ 고쳐 줘");
        let _wait = t.app.idle.open(heard);
        human_typed(&t.app, typing);
        std::fs::File::options()
            .write(true)
            .open(t.dir.path().join("fresh.jsonl"))
            .unwrap()
            .set_modified(SystemTime::now())
            .unwrap();
        let active = t.app.graph.active().unwrap();
        let held = active.into_iter().find(|n| n.id == queued).unwrap();
        assert_eq!(
            t.app.relaunch_or_queue(held).await,
            Ok(Relaunched::AtTurnEnd)
        );

        t.app.end_unhooked_turns().unwrap();
        for name in ["deaf", "queued"] {
            std::fs::remove_file(t.dir.path().join(format!("{name}.jsonl"))).unwrap();
        }
        let found = settle(&t, 6).await;
        assert_eq!(
            found[..4],
            [
                (deaf, false),
                (typing, false),
                (draft, false),
                (queued, false)
            ]
        );
        assert!(found[4..].contains(&(deaf, true)) && found[4..].contains(&(queued, true)));
        for node in [typing, draft] {
            assert_eq!(t.app.agent_state(node), Some(AgentState::Deaf));
            assert!(t.app.relaunches.held(node));
            let file = t.dir.path().join(format!("typed-{}", node.0));
            assert_eq!(
                std::fs::read_to_string(file).unwrap(),
                "",
                "loom types nothing"
            );
        }
        assert_eq!(t.app.agent_state(busy), Some(AgentState::Turn));
        assert_eq!(t.app.agent_state(fresh), Some(AgentState::Turn));
        assert_eq!(t.app.agent_state(heard), Some(AgentState::Listening));
        while t.app.graph.ended(deaf).unwrap().is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        t.app.end_unhooked_turns().unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(missed(&t).len(), 6, "each turn end is noted once");
    }

    fn idle_request(
        t: &Fixture,
        token: &str,
    ) -> tokio::task::JoinHandle<
        Result<axum::Json<crate::daemon::IdleReply>, crate::daemon::AgentError>,
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
            Result<axum::Json<crate::daemon::IdleReply>, crate::daemon::AgentError>,
        >,
    ) -> String {
        let reply = tokio::time::timeout(Duration::from_secs(5), wait)
            .await
            .unwrap()
            .unwrap();
        match reply {
            Ok(axum::Json(crate::daemon::IdleReply::Mail(taken))) => taken[0].body.clone(),
            _ => panic!("the idle wait gave no mail"),
        }
    }

    #[tokio::test]
    async fn a_relaunch_waits_for_the_turn_that_idle_wait_mail_started() {
        let t = fixture();
        let worker = add_worker(&t, "worker");
        let wait = idle_request(&t, "worker");
        tokio::time::sleep(Duration::from_millis(50)).await;
        t.app
            .graph
            .note(t.lead, "worker", Kind::Queue, "implement fix 3", None)
            .unwrap();
        assert_eq!(taken(wait).await, "implement fix 3");
        let active = t.app.graph.active().unwrap();
        let node = active.into_iter().find(|n| n.id == worker).unwrap();
        assert_eq!(
            t.app.relaunch_or_queue(node).await,
            Ok(Relaunched::AtTurnEnd)
        );
        assert_eq!(missed(&t), [], "nothing cut the turn");
    }

    #[tokio::test]
    async fn a_relaunch_waits_while_the_agent_starts_or_turns_and_runs_once_it_listens() {
        let t = fixture();
        let starting = add_worker(&t, "starting");
        let turning = add_worker(&t, "turning");
        let listening = add_worker(&t, "listening");
        t.app.idle.turn_started(turning, "PostToolUse".into());
        let _wait = t.app.idle.open(listening);
        let active = |id: NodeId| {
            let active = t.app.graph.active().unwrap();
            active.into_iter().find(|n| n.id == id).unwrap()
        };
        for node in [starting, turning] {
            assert_eq!(
                t.app.relaunch_or_queue(active(node)).await,
                Ok(Relaunched::AtTurnEnd)
            );
        }
        let now = t.app.relaunch_or_queue(active(listening)).await;
        assert!(now.is_err(), "it ran now and found no session: {now:?}");
        assert_eq!(missed(&t), [(listening, true)]);

        let wait = idle_request(&t, "starting");
        for _ in 0..100 {
            if missed(&t).len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(missed(&t), [(listening, true), (starting, true)]);
        assert!(t.app.at_turn_end(turning), "the turn has not ended");
        wait.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_queued_relaunch_waits_while_the_screen_shows_a_turn_and_runs_once_it_stops() {
        const INTERRUPTED: &str =
            "{\"type\":\"user\",\"message\":{\"content\":\"[Request interrupted by user]\"}}\n";
        let t = fixture();
        let hour = Duration::from_secs(3600);
        let node = |name: &str, transcript: &str, footer: &str| {
            let id = add_worker(&t, name);
            let path = t.dir.path().join(format!("{name}.jsonl"));
            std::fs::write(&path, transcript).unwrap();
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(SystemTime::now() - hour)
                .unwrap();
            t.app
                .graph
                .set_transcript(id, path.to_str().unwrap())
                .unwrap();
            t.app.idle.turn_started(id, "UserPromptSubmit".into());
            t.app.graph.backdate_agent_state(id, hour * 2);
            let mut cmd = portable_pty::CommandBuilder::new("sh");
            cmd.args([
                "-c",
                "stty -icanon -echo; printf '%s' \"$SCREEN\"; exec cat >/dev/null",
            ]);
            cmd.env(
                "SCREEN",
                boxed("❯ ").replace("bypass permissions on", footer),
            );
            host_pty(&t, id, cmd);
            id
        };
        let stopped = node(
            "stopped",
            INTERRUPTED,
            "bypass permissions on (shift+tab to cycle)",
        );
        let working = node(
            "working",
            IN_TOOL,
            "bypass permissions on (shift+tab to cycle) · esc to interrupt",
        );
        let unplanned = node(
            "unplanned",
            INTERRUPTED,
            "bypass permissions on (shift+tab to cycle)",
        );
        for id in [stopped, working] {
            let active = t.app.graph.active().unwrap();
            let held = active.into_iter().find(|n| n.id == id).unwrap();
            assert_eq!(
                t.app.relaunch_or_queue(held).await,
                Ok(Relaunched::AtTurnEnd)
            );
        }

        t.app.end_unhooked_turns().unwrap();
        while !missed(&t).contains(&(stopped, true)) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!t.app.at_turn_end(stopped));
        for _ in 0..5 {
            t.app.end_unhooked_turns().unwrap();
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            missed(&t),
            [(stopped, false), (stopped, true)],
            "a running tool is never cut, and a cut turn without a plan waits for the human"
        );
        assert!(t.app.at_turn_end(working));
        assert_eq!(t.app.agent_state(unplanned), Some(AgentState::Turn));
    }

    #[test]
    fn the_spinner_or_the_footer_shows_a_running_turn() {
        let box_ = "────\n❯ \n────\n  ⏵⏵ bypass permissions on (shift+tab to cycle)\n";
        let idle = format!(
            "● Claude shows esc to interrupt and Thinking… while it works.\n✻ Churned for 18s · done 10:05 PM\n{box_}"
        );
        let stopped = format!("  ⎿  Interrupted · What should Claude do instead?\n{box_}");
        assert!(!turn_shown(&idle));
        assert!(!turn_shown(&stopped));
        let footer = idle.replace("cycle)\n", "cycle) · esc to interrupt\n");
        assert!(turn_shown(&footer));
        let narrow = format!(
            "     (ctrl+b ctrl+b (twice) to run in background)\n· Coalescing… (10s · ↓ 246 tokens)\n  ⎿  Tip: steer Claude\n{box_}"
        );
        assert!(turn_shown(&narrow));
        assert!(turn_shown(&format!("* Ionizing…\n{box_}")));
    }

    #[tokio::test]
    async fn a_cancelled_wake_restart_frees_an_idle_wait_that_came_during_preparation() {
        let t = fixture();
        let fallback = add_worker(&t, "fallback");
        t.app.hold(fallback);
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
    async fn a_lost_idle_wait_relaunches_the_node_at_once_or_at_its_next_event() {
        let t = fixture();
        t.app.watch_agents();
        let node = unheard(&t, "queued", ENDED);
        redrawn(&t, node, "❯ 고쳐 줘", "❯ ");
        drop(t.app.idle.open(node));
        tokio::time::sleep(Duration::from_millis(200)).await;
        let relaunched = || missed(&t).contains(&(node, true));
        let exited = || t.dir.path().join(format!("exited-{}", node.0)).exists();
        assert_eq!(t.app.agent_state(node), Some(AgentState::Deaf));
        assert!(t.app.relaunches.held(node));
        assert!(!relaunched() && !exited(), "no restart over the draft");

        t.app
            .host
            .write_if(node, |_| {
                human_typed(&t.app, node);
                Ok::<_, std::convert::Infallible>(b"\x7f".to_vec())
            })
            .unwrap();
        while claude::input(&t.app.host.screen(node).text) != Some("") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!relaunched(), "typing is no event of the agent");

        let dialog = Some((crate::protocol::WaitKind::Approval, "허용할까요?"));
        t.app
            .graph
            .set_wait("queued", crate::graph::WaitSource::Hook, dialog)
            .unwrap();
        t.app
            .graph
            .set_wait("queued", crate::graph::WaitSource::Hook, None)
            .unwrap();
        std::fs::remove_file(t.dir.path().join("queued.jsonl")).unwrap();
        while !relaunched() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        while t.app.graph.ended(node).unwrap().is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            !t.app.relaunches.held(node),
            "a failed recovery ends the node"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_node_held_by_a_draft_tries_again_when_mail_comes() {
        let t = fixture();
        t.app.watch_agents();
        t.app.watch_sleep();
        let node = unheard(&t, "drafted", ENDED);
        redrawn(&t, node, "❯ 고쳐 줘", "❯ ");
        drop(t.app.idle.open(node));
        let restarted = || missed(&t).contains(&(node, true));
        let mailed = || {
            t.app
                .relaunches
                .mail_since_refused(|id| t.app.graph.newest_mail(id).ok().flatten())
        };
        while !t.app.relaunches.held(node) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(mailed().is_empty());

        t.app
            .graph
            .note(t.lead, "drafted", Kind::Queue, "second", None)
            .unwrap();
        let retried = async {
            while !mailed().is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(5), retried)
            .await
            .expect("the mail tries the recovery again");
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!restarted(), "the draft still holds");
        assert!(t.app.relaunches.held(node));
        assert_eq!(t.app.agent_state(node), Some(AgentState::Deaf));

        t.app
            .host
            .write_if(node, |_| {
                human_typed(&t.app, node);
                Ok::<_, std::convert::Infallible>(b"\x7f".to_vec())
            })
            .unwrap();
        while claude::input(&t.app.host.screen(node).text) != Some("") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!restarted(), "no mail, no try");

        std::fs::remove_file(t.dir.path().join("drafted.jsonl")).unwrap();
        t.app
            .graph
            .note(t.lead, "drafted", Kind::Queue, "third", None)
            .unwrap();
        relaunched(&t, node).await;
    }

    async fn relaunched(t: &Fixture, node: NodeId) {
        for _ in 0..3000 {
            if missed(t).contains(&(node, true)) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("node {} is not relaunched", node.0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_node_left_listening_without_an_idle_wait_relaunches_at_the_sweep() {
        let t = fixture();
        t.app.watch_agents();
        let lost = unheard(&t, "lost", ENDED);
        let heard = unheard(&t, "heard", ENDED);
        for node in [lost, heard] {
            screen(&t, node, "❯ ");
        }
        t.app.feed(lost, AgentEvent::WaitOpened);
        let _wait = t.app.idle.open(heard);
        t.app.hear_lost_waits().unwrap();
        relaunched(&t, lost).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(t.app.agent_state(heard), Some(AgentState::Listening));
        assert!(missed(&t).iter().all(|(node, _)| *node == lost));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_idle_wait_lost_under_a_dialog_relaunches_the_node_when_the_dialog_closes() {
        let t = fixture();
        t.app.watch_agents();
        let node = unheard(&t, "blocked", ENDED);
        screen(&t, node, "❯ ");
        let wait = t.app.idle.open(node);
        let dialog = Some((crate::protocol::WaitKind::Approval, "허용할까요?"));
        let set_wait = |dialog| {
            t.app
                .graph
                .set_wait("blocked", crate::graph::WaitSource::Hook, dialog)
                .unwrap()
        };
        set_wait(dialog);
        drop(wait);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            t.app.agent_state(node),
            Some(AgentState::Blocked {
                before: Box::new(AgentState::Deaf)
            })
        );
        assert!(missed(&t).is_empty(), "no restart under the dialog");
        set_wait(None);
        relaunched(&t, node).await;
    }

    #[cfg(unix)]
    fn hooked(t: &Fixture, node: NodeId) -> u32 {
        let bin = t.dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        crate::fake_exe::write_executable(&bin.join("loom"), "#!/bin/sh\nsleep 60\n");
        let pid = t.dir.path().join(format!("hook-{}", node.0));
        let mut cmd = portable_pty::CommandBuilder::new("sh");
        cmd.args([
            "-c",
            "stty -icanon -echo; PATH=\"$BIN:$PATH\" setsid sh -c 'echo $$ >\"$PID\"; exec loom hook' & printf '%s' \"$SCREEN\"; exec cat >/dev/null",
        ]);
        cmd.env("BIN", &bin);
        cmd.env("PID", &pid);
        cmd.env("SCREEN", boxed("❯ "));
        host_pty(t, node, cmd);
        loop {
            if let Some(pid) = std::fs::read_to_string(&pid)
                .ok()
                .and_then(|text| text.trim().parse().ok())
            {
                return pid;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_lost_idle_wait_with_a_live_hook_relaunches_the_node_at_once() {
        let t = fixture();
        t.app.watch_agents();
        let node = unheard(&t, "hooked", ENDED);
        let hook = hooked(&t, node);
        drop(t.app.idle.open(node));
        std::fs::remove_file(t.dir.path().join("hooked.jsonl")).unwrap();
        relaunched(&t, node).await;
        assert!(
            Path::new(&format!("/proc/{hook}")).exists(),
            "the hook still lives"
        );
        while t.app.graph.ended(node).unwrap().is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let late = t.app.idle.open(node);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!t.app.relaunches.held(node));
        assert_eq!(t.app.agent_state(node), Some(AgentState::Ended));
        assert_eq!(missed(&t), [(node, true)], "one relaunch");
        drop(late);
        let _ = std::process::Command::new("kill")
            .arg(hook.to_string())
            .status();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_node_stored_deaf_relaunches_when_the_daemon_starts_and_its_screen_came() {
        let t = fixture();
        let go = t.dir.path().join("go");
        let mut stored = Vec::new();
        for (name, wait) in [
            ("drawn", ""),
            ("late", "while [ ! -e \"$GO\" ]; do sleep 0.05; done; "),
        ] {
            let node = unheard(&t, name, ENDED);
            t.app.feed(node, AgentEvent::TurnEnded);
            assert_eq!(t.app.agent_state(node), Some(AgentState::Deaf));
            std::fs::remove_file(t.dir.path().join(format!("{name}.jsonl"))).unwrap();
            let mut cmd = portable_pty::CommandBuilder::new("sh");
            cmd.args([
                "-c",
                &format!("stty -icanon -echo; {wait}printf '%s' \"$SCREEN\"; exec cat >/dev/null"),
            ]);
            cmd.env("SCREEN", boxed("❯ "));
            cmd.env("GO", &go);
            crate::node_host::in_process(&t.dir.path().join("pty").join("nodes"), node, cmd);
            stored.push(node);
        }

        let Fixture { dir, app, lead } = t;
        drop(app);
        let db_path = dir.path().join("loom.db");
        let started = Arc::new(
            App::new(
                crate::db::open(&db_path).unwrap(),
                &db_path,
                "127.0.0.1:0".parse().unwrap(),
                None,
                None,
                tokio::sync::mpsc::unbounded_channel().0,
            )
            .unwrap(),
        );
        started.resume().await.unwrap();
        started.watch_agents();
        let t = Fixture {
            dir,
            app: started,
            lead,
        };
        let [drawn, late] = stored[..] else {
            unreachable!()
        };
        relaunched(&t, drawn).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(t.app.agent_state(late), Some(AgentState::Deaf));
        assert!(!t.app.relaunches.held(late), "no guess on an empty screen");
        assert_eq!(missed(&t), [(drawn, true)]);
        std::fs::write(&go, "").unwrap();
        relaunched(&t, late).await;
    }

    #[tokio::test]
    async fn a_cursor_turn_whose_reply_got_no_stop_hook_makes_the_node_deaf_and_relaunches_it() {
        let t = fixture();
        t.app.watch_agents();
        let cursor = |name| {
            let node = add_agent(&t, name, "cursor");
            t.app.idle.turn_started(node, "beforeSubmitPrompt".into());
            node
        };
        let missed_stop = cursor("missed");
        let fresh = cursor("fresh");
        let stepped = cursor("stepped");
        let silent = cursor("silent");
        let old = Instant::now() - STOP_HOOK_GRACE * 2;
        let late = cursor("late");
        for node in [missed_stop, stepped] {
            t.app.turn_ends.said_at(node, "끝".into(), None, old);
        }
        t.app.turn_ends.said(fresh, "끝".into(), None);
        t.app.turn_ends.stepped(stepped, None);
        t.app.turn_ends.stepped(late, Some("A"));
        t.app.turn_ends.stepped(late, Some("B"));
        t.app.turn_ends.said_at(late, "끝".into(), Some("A"), old);
        let followup = cursor("followup");
        t.app.turn_ends.stepped(followup, Some("A"));
        t.app.turn_ends.ended(followup, || false, || false);
        t.app
            .turn_ends
            .said_at(followup, "끝".into(), Some("B"), old);
        let stopped_first = cursor("stoppedFirst");
        t.app.turn_ends.stepped(stopped_first, Some("A"));
        t.app.turn_ends.ended(stopped_first, || false, || false);
        t.app.turn_ends.stopped(stopped_first, Some("B"));
        t.app.turn_ends.ended(stopped_first, || false, || false);
        t.app
            .turn_ends
            .said_at(stopped_first, "끝".into(), Some("B"), old);
        let older = cursor("older");
        for turn in ["A", "B"] {
            t.app.turn_ends.stepped(older, Some(turn));
            t.app.turn_ends.ended(older, || false, || false);
        }
        t.app.turn_ends.said_at(older, "끝".into(), Some("A"), old);
        let retired = cursor("retired");
        t.app.turn_ends.stepped(retired, Some("A"));
        t.app.turn_ends.ended(retired, || false, || false);
        t.app
            .turn_ends
            .said_at(retired, "끝".into(), Some("A"), old);
        for _ in 0..3 {
            t.app.end_unhooked_turns().unwrap();
        }
        relaunched(&t, missed_stop).await;
        relaunched(&t, followup).await;
        let mut noted: Vec<_> = missed(&t)
            .into_iter()
            .filter(|(_, relaunch)| !relaunch)
            .map(|(node, _)| node)
            .collect();
        noted.sort_by_key(|node| node.0);
        assert_eq!(noted, vec![missed_stop, followup]);
        for node in [fresh, stepped, silent, late, retired, stopped_first, older] {
            assert_eq!(t.app.agent_state(node), Some(AgentState::Turn));
        }
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
                    tokio::sync::mpsc::unbounded_channel().0,
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
        app.feed(worker, AgentEvent::WaitOpened);
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
