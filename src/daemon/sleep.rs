use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Notify;
use tokio::sync::broadcast::error::RecvError;

use super::App;
use super::relaunch::{EXIT_WAIT, Guard, Stop, profile_of};
use crate::graph::{
    Active, AgentChange, AgentEvent, AgentState, Attempt, Entry, IdleEnd, NodeId, Phase, Plan,
};
use crate::process_tree::Snapshot;
use crate::profile::{self, Profile, claude};
use crate::protocol::ServerLink;
use crate::screen::Screen;

const DONE_GRACE: Duration = Duration::from_secs(600);
const STOP_HOOK_GRACE: Duration = Duration::from_secs(60);
const RUNNING_STATUS: &str = "esc to interrupt";

type Recovery = (Active, Plan, Guard, fn(&App, NodeId));

#[derive(Default)]
pub struct Sleeps {
    resting: Mutex<HashMap<NodeId, Instant>>,
    stopping: Mutex<HashSet<NodeId>>,
    knocked: Mutex<HashSet<NodeId>>,
    settled: Mutex<HashMap<NodeId, Instant>>,
    changed: Notify,
}

impl Sleeps {
    pub fn stopping(&self, node: NodeId) -> bool {
        self.stopping.lock().unwrap().contains(&node)
    }

    pub fn unless_stopping<T>(&self, node: NodeId, take: impl FnOnce() -> T) -> Option<T> {
        let stopping = self.stopping.lock().unwrap();
        (!stopping.contains(&node)).then(take)
    }

    pub(super) fn begin(&self, node: NodeId) -> bool {
        self.stopping.lock().unwrap().insert(node)
    }

    fn cancel(&self, node: NodeId) -> bool {
        self.stopping.lock().unwrap().remove(&node)
    }

    pub fn exited(&self, node: NodeId) {
        self.stopping.lock().unwrap().remove(&node);
        self.resting.lock().unwrap().remove(&node);
        self.settled.lock().unwrap().remove(&node);
    }

    pub fn knocked(&self, node: NodeId) {
        self.knocked.lock().unwrap().insert(node);
        self.changed();
    }

    pub fn changed(&self) {
        self.changed.notify_one();
    }

    pub fn settled(&self, node: NodeId) {
        self.settled.lock().unwrap().insert(node, now());
        self.changed();
    }

    fn settle_due(&self, node: NodeId, now: Instant) -> Option<Instant> {
        let mut settled = self.settled.lock().unwrap();
        let due = *settled.get(&node)? + STOP_HOOK_GRACE;
        if due > now {
            return Some(due);
        }
        settled.remove(&node);
        None
    }

    fn rest_again(&self, node: NodeId) -> bool {
        let cancelled = self.cancel(node);
        if cancelled {
            self.resting.lock().unwrap().insert(node, now());
        }
        cancelled
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

    fn next_due(&self, now: Instant) -> Option<Instant> {
        self.resting
            .lock()
            .unwrap()
            .values()
            .map(|since| *since + DONE_GRACE)
            .filter(|due| *due > now)
            .min()
    }
}

fn now() -> Instant {
    tokio::time::Instant::now().into_std()
}

async fn until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at.into()).await,
        None => std::future::pending().await,
    }
}

impl App {
    pub(super) fn watch_sleep(self: &Arc<Self>) {
        let app = self.clone();
        tokio::spawn(async move {
            loop {
                let mut mail = pin!(app.graph.mail_changed());
                mail.as_mut().enable();
                let mut changed = pin!(app.sleeps.changed.notified());
                changed.as_mut().enable();
                if let Err(e) = app.wake_sleepers() {
                    eprintln!("loom: 잠든 노드를 보지 못했다: {e}");
                }
                app.recover_held_with_mail();
                let turns = app.end_unhooked_turns().unwrap_or_else(|e| {
                    eprintln!("loom: stop 훅이 닿지 않은 노드를 보지 못했다: {e}");
                    None
                });
                let resting = app.sleep_resting().unwrap_or_else(|e| {
                    eprintln!("loom: 쉬는 노드를 보지 못했다: {e}");
                    None
                });
                tokio::select! {
                    () = mail => {}
                    () = changed => {}
                    () = until(turns.into_iter().chain(resting).min()) => {}
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
        self.sleeps.changed();
        match change.to {
            AgentState::Deaf => {}
            AgentState::Blocked { .. } => return,
            _ => return self.attempt_cleared(change.node, Attempt::Deaf),
        }
        let lost = match change.by {
            AgentEvent::WaitClosed {
                by: IdleEnd::Gone | IdleEnd::Error,
            } => change.from == AgentState::Listening,
            AgentEvent::ServerLost | AgentEvent::DialogCleared => true,
            AgentEvent::TurnEnded => false,
            _ => self.held(change.node),
        };
        if lost {
            self.recover(change.node);
        }
    }

    fn recover_stored(self: &Arc<Self>) -> rusqlite::Result<()> {
        for node in self.graph.active()? {
            let id = node.id;
            if self.agent_state(id) != Some(AgentState::Deaf)
                || !self.graph.mail_since_attempt(id, Attempt::Deaf)?
            {
                continue;
            }
            self.recover(id);
        }
        Ok(())
    }

    fn recover(self: &Arc<Self>, id: NodeId) {
        if let Some(mut screen) = self.host.attach(id).filter(|a| a.backlog.is_empty()) {
            let app = self.clone();
            tokio::spawn(async move {
                if !matches!(screen.output.recv().await, Err(RecvError::Closed)) {
                    app.recover(id);
                }
            });
            return;
        }
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
                Err(e) if app.graph.ended(id).is_ok_and(|ended| ended.is_none()) => {
                    eprintln!(
                        "loom: 듣지 못하는 노드 {}를 살리지 못했다. 옛 프로세스를 두고 다음 사건에 다시 해 본다: {e}",
                        id.0
                    );
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

    pub(super) fn draft_ended(self: &Arc<Self>, id: NodeId) {
        match self.graph.end_draft(id) {
            Ok(true) if self.held(id) => self.recover(id),
            Ok(_) => {}
            Err(e) => eprintln!("loom: 노드 {}의 입력 중 표시를 지우지 못했다: {e}", id.0),
        }
    }

    fn refused(&self, id: NodeId) {
        if let Err(e) = self.graph.attempted(id, Attempt::Deaf) {
            eprintln!("loom: 노드 {}의 살리기 시도를 적지 못했다: {e}", id.0);
        }
    }

    fn held(&self, id: NodeId) -> bool {
        self.graph
            .attempt_held(id, Attempt::Deaf)
            .inspect_err(|e| eprintln!("loom: 노드 {}의 살리기 시도를 읽지 못했다: {e}", id.0))
            .unwrap_or(false)
    }

    fn attempt_cleared(&self, id: NodeId, cause: Attempt) {
        if let Err(e) = self.graph.attempt_cleared(id, cause) {
            eprintln!("loom: 노드 {}의 살리기 시도를 지우지 못했다: {e}", id.0);
        }
    }

    fn recover_held_with_mail(self: &Arc<Self>) {
        let mailed = self
            .graph
            .mailed_since_attempt(Attempt::Deaf)
            .unwrap_or_else(|e| {
                eprintln!("loom: 살리기를 미룬 노드를 보지 못했다: {e}");
                Vec::new()
            });
        for id in mailed {
            self.refused(id);
            self.recover(id);
        }
    }

    fn recovering(self: &Arc<Self>, id: NodeId) -> rusqlite::Result<Option<Recovery>> {
        if self.agent_state(id) != Some(AgentState::Deaf)
            || self.sleeps.stopping(id)
            || self.relaunching(id)
        {
            return Ok(None);
        }
        let Some(node) = self.graph.active()?.into_iter().find(|n| n.id == id) else {
            return Ok(None);
        };
        if profile_of(&node).is_none() {
            return Ok(None);
        }
        let guard = self.restart_guard(&node)?;
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
        let mut waking = Vec::new();
        for id in self.graph.sleepers_with_mail()? {
            if self.graph.mail_since_attempt(id, Attempt::Wake)? {
                waking.push(id);
            }
        }
        let unattached = self.host.unattached();
        for node in self.graph.active()? {
            let id = node.id;
            if !unattached.contains(&id) {
                continue;
            }
            if (knocked.contains(&id) || self.graph.mail_since_attempt(id, Attempt::Wake)?)
                && self.host.take_unattached(id)
            {
                self.graph.attempted(id, Attempt::Wake)?;
                let app = self.clone();
                tokio::spawn(async move {
                    if let Err(e) = app.resume_node(node).await {
                        eprintln!("loom: 노드 {}에 다시 붙지 못했다: {e}", id.0);
                    }
                });
            }
        }
        for id in knocked {
            if !waking.contains(&id) && self.graph.asleep(id)? {
                waking.push(id);
            }
        }
        for id in waking {
            self.graph.attempted(id, Attempt::Wake)?;
            let app = self.clone();
            tokio::spawn(async move {
                match app.relaunch_by_id(id, Plan::Resume).await {
                    Ok(_) => app.attempt_cleared(id, Attempt::Wake),
                    Err(e) => eprintln!("loom: 잠든 노드 {}를 깨우지 못했다: {e:?}", id.0),
                }
            });
        }
        Ok(())
    }

    pub(super) fn hear_lost_wait(&self, id: NodeId) -> rusqlite::Result<()> {
        let Some(node) = self.graph.active()?.into_iter().find(|n| n.id == id) else {
            return Ok(());
        };
        let hooked = profile_of(&node).is_some_and(Profile::hooked);
        if hooked && !self.sleeps.stopping(id) {
            self.idle.lose_unheld(id);
        }
        Ok(())
    }

    fn end_unhooked_turns(self: &Arc<Self>) -> rusqlite::Result<Option<Instant>> {
        let mut next = None;
        for node in self.graph.active()? {
            let id = node.id;
            let profile = profile_of(&node);
            if !profile.is_some_and(Profile::ends_turns_by_stop_hook)
                || self.sleeps.stopping(id)
                || self.agent_state(id) != Some(AgentState::Turn)
            {
                continue;
            }
            let quiet = match profile {
                Some(Profile::Claude) => self.transcript_quiet(&node)?,
                _ => self.turn_ends.reply_age(id),
            };
            let Some(quiet) = quiet else {
                next = next
                    .into_iter()
                    .chain(self.sleeps.settle_due(id, now()))
                    .min();
                continue;
            };
            if let Some(left) = STOP_HOOK_GRACE.checked_sub(quiet)
                && !left.is_zero()
            {
                next = next.into_iter().chain([now() + left]).min();
                continue;
            }
            if profile == Some(Profile::Claude) && !self.claude_turn_ended(&node)? {
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
        Ok(next)
    }

    fn transcript_quiet(&self, node: &Active) -> rusqlite::Result<Option<Duration>> {
        let Some(path) = self.transcript(node)? else {
            return Ok(None);
        };
        let Some(began) = self.graph.turn_began(node.id)? else {
            return Ok(None);
        };
        Ok(std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .filter(|at| *at > began)
            .map(|at| at.elapsed().unwrap_or_default()))
    }

    fn claude_turn_ended(&self, node: &Active) -> rusqlite::Result<bool> {
        let ended = self.at_turn_end(node.id)
            || self
                .transcript(node)?
                .is_some_and(|path| claude::turn_ended(&path));
        Ok(ended && !turn_shown(&self.host.screen(node.id).text))
    }

    pub(super) fn transcript(&self, node: &Active) -> rusqlite::Result<Option<PathBuf>> {
        let Some(transcript) = &node.transcript else {
            return Ok(None);
        };
        let place = self.graph.place_of(node.id)?;
        Ok(Some(crate::wsl::from_node(
            PathBuf::from(transcript),
            Path::new(&place.worktree),
        )))
    }

    fn restart_guard(self: &Arc<Self>, node: &Active) -> rusqlite::Result<Guard> {
        let (app, id, profile) = (self.clone(), node.id, profile_of(node));
        let path = self.transcript(node)?;
        let modified = |path: &Option<PathBuf>| {
            path.as_ref()
                .and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        };
        let changed = modified(&path);
        Ok(Arc::new(move |screen: &Screen| {
            let quiet = match profile {
                Some(Profile::Claude) => {
                    claude::input(&screen.undimmed) == Some("") && !turn_shown(&screen.text)
                }
                _ => true,
            };
            quiet
                && modified(&path) == changed
                && app.agent_state(id) == Some(AgentState::Deaf)
                && !app.graph.drafted(id).unwrap_or(true)
                && profile::prompt(profile, screen).is_none()
        }))
    }

    fn sleep_resting(self: &Arc<Self>) -> rusqlite::Result<Option<Instant>> {
        let children = self.graph.done_children()?;
        let resting: Vec<Active> = self
            .graph
            .active()?
            .into_iter()
            .filter(|n| children.contains(&n.id) && self.resting(n))
            .collect();
        let ids: Vec<NodeId> = resting.iter().map(|n| n.id).collect();
        let now = now();
        let due = self.sleeps.due(&ids, now);
        for node in resting {
            if due.contains(&node.id) && self.start_sleep(node.id) {
                let app = self.clone();
                tokio::spawn(async move { app.put_to_sleep(node).await });
            }
        }
        Ok(self.sleeps.next_due(now))
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

    pub(super) fn sleep_again_after_start(self: &Arc<Self>, node: Active) {
        let app = self.clone();
        tokio::spawn(async move { app.put_to_sleep(node).await });
    }

    fn start_sleep(&self, id: NodeId) -> bool {
        if !self.sleeps.begin(id) {
            return false;
        }
        if let Err(e) = self.graph.begin_sleep(id) {
            eprintln!("loom: 노드 {}의 재우기 표시를 쓰지 못했다: {e}", id.0);
            self.sleeps.cancel(id);
            return false;
        }
        true
    }

    fn sleep_cancelled(&self, id: NodeId) -> bool {
        let cancelled = self.sleeps.rest_again(id);
        if cancelled && let Err(e) = self.graph.cancel_sleep(id) {
            eprintln!("loom: 노드 {}의 재우기 표시를 지우지 못했다: {e}", id.0);
        }
        cancelled
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
            self.sleep_cancelled(id);
            self.graph.wake_mail();
            return;
        }
        eprintln!("loom: 쉬는 완료 노드 {}를 재운다", id.0);
        let snapshot = Snapshot::now();
        snapshot.kill_trees(self.background_shells(&snapshot, &node));
        self.idle.end(id, IdleEnd::Sleep);
        self.host.stop(id);
        tokio::time::sleep(EXIT_WAIT).await;
        if self.sleep_cancelled(id) {
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
