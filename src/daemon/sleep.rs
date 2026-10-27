use std::collections::{HashMap, HashSet};
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Notify;
use tokio::sync::broadcast::error::RecvError;

use super::App;
use super::relaunch::{EXIT_WAIT, Guard, Stop, profile_of};
use crate::graph::{
    Active, AgentChange, AgentEvent, AgentState, Attempt, IdleEnd, NodeId, Phase, Plan,
    ProcessOutcome,
};
use crate::process_tree::Snapshot;
use crate::profile::{self, Profile};
use crate::screen::Screen;

const DONE_GRACE: Duration = Duration::from_secs(600);

type Recovery = (Active, Plan, Guard, fn(&App, NodeId));

#[derive(Default)]
pub struct Sleeps {
    resting: Mutex<HashMap<NodeId, Instant>>,
    knocked: Mutex<HashSet<NodeId>>,
    changed: Notify,
}

impl Sleeps {
    pub fn exited(&self, node: NodeId) {
        self.resting.lock().unwrap().remove(&node);
    }

    pub fn knocked(&self, node: NodeId) {
        self.knocked.lock().unwrap().insert(node);
        self.changed();
    }

    pub fn changed(&self) {
        self.changed.notify_one();
    }

    fn rest_again(&self, node: NodeId) {
        self.resting.lock().unwrap().insert(node, now());
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
                let resting = app.sleep_resting().unwrap_or_else(|e| {
                    eprintln!("loom: 쉬는 노드를 보지 못했다: {e}");
                    None
                });
                tokio::select! {
                    () = mail => {}
                    () = changed => {}
                    () = app.distro_up() => app.attempts_forgotten(Attempt::Wake),
                    () = until(resting) => {}
                }
            }
        });
    }

    async fn distro_up(&self) {
        match &self.distros {
            Some(distros) => distros.up().await,
            None => std::future::pending().await,
        }
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
            Some(AgentState::Deaf) => {}
            Some(AgentState::Blocked { .. }) => return,
            _ => return self.attempt_cleared(change.node, Attempt::Deaf),
        }
        let lost = match change.by {
            AgentEvent::WaitClosed {
                by: IdleEnd::Gone | IdleEnd::Error,
            } => change.from == Some(AgentState::Listening),
            AgentEvent::Lost | AgentEvent::DialogCleared => true,
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

    pub(super) fn turn_stopped(self: &Arc<Self>, id: NodeId) -> rusqlite::Result<()> {
        self.graph.attempted(id, Attempt::Deaf)?;
        if self.graph.waking_mail(id)? {
            self.recover(id);
        }
        Ok(())
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

    fn attempts_forgotten(&self, cause: Attempt) {
        if let Err(e) = self.graph.attempts_forgotten(cause) {
            eprintln!("loom: 살리기 시도를 지우지 못했다: {e}");
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
            self.recover(id);
        }
    }

    fn recovering(self: &Arc<Self>, id: NodeId) -> rusqlite::Result<Option<Recovery>> {
        if self.agent_state(id) != Some(AgentState::Deaf)
            || self.graph.sleeping(id)?
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
        let guard = self.restart_guard(&node);
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
        let unreached = self.graph.unreached_hosts()?;
        for node in self.graph.active()? {
            let id = node.id;
            if !unreached.contains(&id) {
                continue;
            }
            if (knocked.contains(&id) || self.graph.mail_since_attempt(id, Attempt::Wake)?)
                && self.graph.claim_unreached(id)?
            {
                self.graph.attempted(id, Attempt::Wake)?;
                let app = self.clone();
                tokio::spawn(async move {
                    if let Err(e) = app.resume_node(node).await {
                        eprintln!("loom: 노드 {}에 다시 붙지 못했다: {e}", id.0);
                    } else if !app.graph.unreached_hosts().is_ok_and(|u| u.contains(&id)) {
                        app.attempt_cleared(id, Attempt::Wake);
                        app.sleeps.changed();
                    }
                });
            }
        }
        for id in self.graph.hosts_gone()? {
            if knocked.contains(&id) || self.graph.mail_since_attempt(id, Attempt::Wake)? {
                waking.push(id);
            }
        }
        for id in knocked {
            if !waking.contains(&id) && self.graph.process(id)? == Some(ProcessOutcome::Asleep) {
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
        if self.graph.active()?.iter().any(|n| n.id == id) && !self.graph.sleeping(id)? {
            self.graph.end_old_idle(id)?;
        }
        Ok(())
    }

    fn restart_guard(self: &Arc<Self>, node: &Active) -> Guard {
        let (app, id, profile) = (self.clone(), node.id, profile_of(node));
        Arc::new(move |screen: &Screen| {
            profile::input_seen(profile, screen).is_none_or(|seen| seen.empty && !seen.turn_shown)
                && app.agent_state(id) == Some(AgentState::Deaf)
                && !app.graph.drafted(id).unwrap_or(true)
                && profile::prompt(profile, screen).is_none()
        })
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
        profile_of(node).is_some_and(Profile::sleeps) && node.session.is_some()
    }

    fn listening(&self, node: NodeId) -> bool {
        self.agent_state(node) == Some(AgentState::Listening)
    }

    pub(super) fn sleep_again_after_start(self: &Arc<Self>, node: Active) {
        let app = self.clone();
        tokio::spawn(async move { app.put_to_sleep(node).await });
    }

    fn start_sleep(&self, id: NodeId) -> bool {
        self.graph.begin_sleep(id).unwrap_or_else(|e| {
            eprintln!("loom: 노드 {}의 재우기 표시를 쓰지 못했다: {e}", id.0);
            false
        })
    }

    fn sleep_cancelled(&self, id: NodeId) -> bool {
        let cancelled = self.graph.cancel_sleep(id).unwrap_or_else(|e| {
            eprintln!("loom: 노드 {}의 재우기 표시를 지우지 못했다: {e}", id.0);
            false
        });
        if cancelled {
            self.sleeps.rest_again(id);
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
            true => self.end_detached(&node).await,
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
