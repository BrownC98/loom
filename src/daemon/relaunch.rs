use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path as UrlPath, State};
use axum::http::HeaderMap;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use ts_rs::TS;

use super::{AgentError, App, Problem, RelaunchRequest, for_agent, internal};
use crate::accounts::{self, Site};
use crate::graph::{
    Active, AgentEvent, AgentState, AgentSwitch, Entry, IdleEnd, NewAgent, NodeId, Phase, Plan, Who,
};
use crate::process_tree::Snapshot;
use crate::profile::codex;
use crate::profile::{self, Profile, Start};
use crate::protocol::{AgentId, Delivery, Kind, Model, ModelChoice, Provider, Refusal, ServerLink};
use crate::screen::Screen;
use crate::settings::SettingsView;
use crate::skills::Position;

pub(super) const EXIT_WAIT: Duration = Duration::from_secs(10);
const CLAUDE_CLI_WAIT: Duration = Duration::from_secs(20);
const CONTINUE_PROMPT: &str =
    "Your account was switched after a usage limit. Continue the task where you stopped.";
const WOKEN_NOTICE: &str = "loom ended your process while you were done and idle, to free memory. It started your process again in the same session. \
     Messages that came for you arrive next.";
const REVIVED_NOTICE: &str = "Your process ended, and loom started it again in the same session. \
     Messages sent while it was ended did not arrive. Run `loom status`, then continue your work.";
pub(super) const HOOK_COMMANDS: [&str; 2] = ["loom hook", "loom cursor-hook"];
const STOPPED_LINE_CHARS: usize = 200;
const CLAUDE_TRANSCRIPT_DEPTH: usize = 3;
const CODEX_ROLLOUT_DEPTH: usize = 5;

#[derive(Default)]
pub struct Relaunches {
    exiting: Mutex<HashMap<NodeId, oneshot::Sender<()>>>,
    refused: Mutex<HashMap<NodeId, Held>>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Held {
    Human,
    Hook,
}

pub(super) type Guard = Arc<dyn Fn(&Screen) -> bool + Send + Sync>;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Stop {
    Stopped,
    Kept,
}

impl Relaunches {
    pub fn exited(&self, node: NodeId) -> bool {
        match self.exiting.lock().unwrap().remove(&node) {
            Some(done) => {
                let _ = done.send(());
                true
            }
            None => false,
        }
    }

    pub(super) fn exiting(&self, node: NodeId) -> bool {
        self.exiting.lock().unwrap().contains_key(&node)
    }

    pub(super) fn refused(&self, node: NodeId, by: Held) {
        self.refused.lock().unwrap().insert(node, by);
    }

    pub(super) fn held(&self, node: NodeId) -> Option<Held> {
        self.refused.lock().unwrap().get(&node).copied()
    }

    pub(super) fn heard(&self, node: NodeId) {
        self.refused.lock().unwrap().remove(&node);
    }
}

/// 다시 띄우기를 부른 결과.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Relaunched {
    /// 쉬던 노드다. 새 프로세스가 떴다.
    Now,
    /// 턴 중인 노드다. 그 턴이 끝나면 다시 띄운다.
    AtTurnEnd,
}

pub fn routes() -> Router<Arc<App>> {
    Router::new().route(
        "/api/nodes/{id}/relaunch",
        get(relaunch_queued).post(relaunch_node),
    )
}

async fn relaunch_queued(State(app): State<Arc<App>>, UrlPath(id): UrlPath<u32>) -> Json<bool> {
    Json(app.holds_mail(NodeId(id)))
}

async fn relaunch_node(
    State(app): State<Arc<App>>,
    UrlPath(id): UrlPath<u32>,
    to: Option<Json<NewAgent>>,
) -> Result<Json<Relaunched>, Problem> {
    let plan = to.map_or(Plan::Resume, |Json(to)| Plan::Switch(to));
    app.relaunch_by_id(NodeId(id), plan).await.map(Json)
}

pub(super) async fn agent_relaunch(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<RelaunchRequest>,
) -> Result<Json<Relaunched>, AgentError> {
    let caller = app.caller(&headers)?;
    let via = super::gate::via(&headers, "/api/agent/relaunch");
    let to = req.name.clone().unwrap_or_else(|| format!("#{}", caller.0));
    let relaunched = relaunch(&app, caller, req).await;
    super::traced(&app, caller, super::Attempt::verb(&to), &via, relaunched)
}

pub(super) async fn agent_relaunch_unserved(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
) -> Result<Json<Relaunched>, AgentError> {
    let caller = app.caller(&headers)?;
    let via = super::gate::via(&headers, "/api/agent/relaunch-unserved");
    let relaunched = app.relaunch_unserved(caller).await;
    let to = format!("#{}", caller.0);
    super::traced(&app, caller, super::Attempt::verb(&to), &via, relaunched)
}

async fn relaunch(
    app: &Arc<App>,
    caller: NodeId,
    req: RelaunchRequest,
) -> Result<Relaunched, Problem> {
    let node = match &req.name {
        None => caller,
        Some(name) => {
            let (id, _) = app.graph.related(caller, name)?;
            let parent = app.graph.relations(id)?.parent;
            if parent.map(|p| p.id) != Some(caller) {
                return Err(Refusal::NotChild(name.clone()).into());
            }
            id
        }
    };
    let plan = req.to.map_or(Plan::Resume, Plan::Switch);
    app.relaunch_by_id(node, plan).await
}

impl App {
    pub(super) fn holds_mail(&self, node: NodeId) -> bool {
        self.pending(node).is_some()
    }

    pub(super) fn at_turn_end(&self, node: NodeId) -> bool {
        self.pending(node) == Some(Phase::AtTurnEnd)
    }

    fn pending(&self, node: NodeId) -> Option<Phase> {
        match self.graph.relaunch_plan(node) {
            Ok(plan) => plan.map(|(phase, _)| phase),
            Err(e) => {
                eprintln!(
                    "loom: 노드 {}의 다시 띄우기 계획을 읽지 못했다: {e}",
                    node.0
                );
                None
            }
        }
    }

    #[cfg(test)]
    pub(super) fn hold(&self, node: NodeId) {
        self.change_plan(node, |_| (Some((Phase::Running, Plan::Resume)), ()));
    }

    fn drop_plan(&self, node: NodeId) {
        self.change_plan(node, |_| (None, ()));
    }

    fn change_plan<R: Default>(
        &self,
        node: NodeId,
        decide: impl FnOnce(crate::graph::Planned) -> (Option<(Phase, Plan)>, R),
    ) -> R {
        self.graph
            .plan_relaunch(node, decide)
            .inspect_err(|e| {
                eprintln!(
                    "loom: 노드 {}의 다시 띄우기 계획을 쓰지 못했다: {e}",
                    node.0
                )
            })
            .unwrap_or_default()
    }

    pub(super) fn process_ended(&self, node: NodeId) {
        let dropped = self.change_plan(node, |now| match now.pending {
            Some((Phase::AtTurnEnd, _)) => (None, true),
            kept => (kept, false),
        });
        if dropped {
            eprintln!(
                "loom: 노드 {}의 프로세스가 끝나 턴 끝 다시 띄우기를 지운다",
                node.0
            );
        }
    }

    pub(super) fn take_mail(
        &self,
        node: NodeId,
        kinds: &[Kind],
        limit_bytes: usize,
    ) -> Result<Vec<Delivery>, Refusal> {
        if self.holds_mail(node) {
            return Ok(Vec::new());
        }
        self.sleeps
            .unless_stopping(node, || self.graph.take_waking(node, kinds, limit_bytes))
            .unwrap_or(Ok(Vec::new()))
    }

    pub(super) async fn next_mail(
        &self,
        node: NodeId,
        kinds: &[Kind],
        limit_bytes: usize,
    ) -> Result<Vec<Delivery>, Refusal> {
        loop {
            let mut changed = pin!(self.graph.mail_changed());
            changed.as_mut().enable();
            let taken = self.take_mail(node, kinds, limit_bytes)?;
            if !taken.is_empty() {
                return Ok(taken);
            }
            changed.await;
        }
    }

    pub(super) fn feed(&self, node: NodeId, event: AgentEvent) {
        if let Err(e) = self.graph.agent_event(node, event) {
            eprintln!("loom: 노드 {}의 에이전트 상태를 쓰지 못했다: {e}", node.0);
        }
    }

    pub(super) fn agent_state(&self, node: NodeId) -> Option<AgentState> {
        self.graph
            .agent_state(node)
            .inspect_err(|e| eprintln!("loom: 노드 {}의 에이전트 상태를 읽지 못했다: {e}", node.0))
            .ok()
            .flatten()
    }

    pub(super) fn busy(&self, node: NodeId) -> bool {
        self.agent_state(node).is_some_and(|s| s.busy())
    }

    pub(super) async fn relaunch_by_id(
        self: &Arc<Self>,
        id: NodeId,
        plan: Plan,
    ) -> Result<Relaunched, Problem> {
        let found = self
            .graph
            .active()
            .map_err(internal)?
            .into_iter()
            .find(|n| n.id == id);
        let Some(node) = found else {
            return self.revive_ended(id, plan).await;
        };
        if profile_of(&node).is_none() {
            return Err(Refusal::NoResume(node.name).into());
        }
        let plan = self.chosen(&node, plan)?;
        self.queue(node, plan).await
    }

    fn chosen(&self, node: &Active, plan: Plan) -> Result<Plan, Problem> {
        let Plan::Switch(NewAgent { agent, choice }) = plan else {
            return Ok(plan);
        };
        if profile::profile_of(&agent).is_none() {
            return Err(Refusal::NoLink(agent.0).into());
        }
        let choice = match &node.role {
            Some(role) => self
                .settings
                .get()
                .map_err(internal)?
                .roles
                .choice_for(role, &agent, choice),
            None => choice,
        };
        profile::check_choice(&agent, &choice)?;
        Ok(Plan::Switch(NewAgent { agent, choice }))
    }

    async fn revive_ended(self: &Arc<Self>, id: NodeId, plan: Plan) -> Result<Relaunched, Problem> {
        let node = self
            .graph
            .ended(id)
            .map_err(internal)?
            .ok_or_else(|| Refusal::Ended(format!("#{}", id.0)))?;
        if profile_of(&node).is_none() {
            return Err(Refusal::NoResume(node.name).into());
        }
        let plan = self.chosen(&node, plan)?;
        let asleep = self.graph.asleep(id).map_err(internal)?;
        let notice = match asleep {
            true => WOKEN_NOTICE,
            false => REVIVED_NOTICE,
        };
        let running = self.change_plan(id, |now| match now.pending {
            Some((Phase::Running, kept)) => (Some((Phase::Running, kept)), true),
            _ => (Some((Phase::Running, plan.clone())), false),
        });
        if running {
            return match plan {
                Plan::Resume => Ok(Relaunched::Now),
                Plan::Switch(_) => Err(Refusal::Relaunching(node.name).into()),
            };
        }
        let who = Who {
            id: Some(id),
            name: node.name.clone(),
        };
        let switched = switch_of(&node, &plan);
        let app = self.clone();
        let result = tokio::spawn(async move {
            match plan {
                Plan::Resume => app.revive(node, notice).await,
                Plan::Switch(to) => {
                    let prepared = app.prepare_other(&node, &to).await?;
                    if let Err(e) = app.stop_old_claude(&node).await {
                        prepared.discarder()();
                        return Err(e);
                    }
                    app.run_other(node.id, prepared, &to).await
                }
            }
        })
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
        self.drop_plan(id);
        if asleep
            && result.is_err()
            && let Err(e) = self.graph.sleep_lost(id)
        {
            eprintln!("loom: 노드 {}를 exited로 바꾸지 못했다: {e}", id.0);
        }
        self.record_relaunch(who, switched, &result);
        result.map(|()| Relaunched::Now).map_err(internal)
    }

    pub(super) async fn relaunch_or_queue(
        self: &Arc<Self>,
        node: Active,
    ) -> Result<Relaunched, String> {
        self.queue(node, Plan::Resume)
            .await
            .map_err(|e| for_agent(e).1)
    }

    async fn queue(self: &Arc<Self>, node: Active, plan: Plan) -> Result<Relaunched, Problem> {
        let queued = self
            .graph
            .plan_relaunch(node.id, |now| {
                if now.ended {
                    return (now.pending, Queued::Ended);
                }
                match (now.pending, &plan) {
                    (Some((Phase::Running, kept)), Plan::Switch(_)) => {
                        (Some((Phase::Running, kept)), Queued::Refused)
                    }
                    (Some((phase, kept)), Plan::Resume) => {
                        (Some((phase, kept)), Queued::Kept(phase))
                    }
                    (Some((phase, _)), Plan::Switch(_)) => {
                        (Some((phase, plan.clone())), Queued::Kept(phase))
                    }
                    (None, _) => {
                        let phase = match now.busy {
                            true => Phase::AtTurnEnd,
                            false => Phase::Running,
                        };
                        (Some((phase, plan.clone())), Queued::Added(phase))
                    }
                }
            })
            .map_err(internal)?;
        match queued {
            Queued::Ended => self.revive_ended(node.id, plan).await,
            Queued::Refused => Err(Refusal::Relaunching(node.name).into()),
            Queued::Kept(Phase::AtTurnEnd) | Queued::Added(Phase::AtTurnEnd) => {
                Ok(Relaunched::AtTurnEnd)
            }
            Queued::Kept(Phase::Running) => Ok(Relaunched::Now),
            Queued::Added(Phase::Running) => {
                let app = self.clone();
                tokio::spawn(async move { app.relaunch(node, plan, None).await })
                    .await
                    .map_err(internal)?
                    .map_err(internal)?;
                Ok(Relaunched::Now)
            }
        }
    }

    pub(super) async fn relaunch_unserved(
        self: &Arc<Self>,
        id: NodeId,
    ) -> Result<Relaunched, Problem> {
        self.feed(id, AgentEvent::VersionMismatch);
        let found = self
            .graph
            .active()
            .map_err(internal)?
            .into_iter()
            .find(|n| n.id == id);
        let Some(node) = found else {
            return self.revive_ended(id, Plan::Resume).await;
        };
        if profile_of(&node).is_none() {
            return Err(Refusal::NoResume(node.name).into());
        }
        let claimed = self
            .graph
            .plan_relaunch(id, |now| match (now.ended, now.pending) {
                (true, kept) => (kept, Queued::Ended),
                (false, Some((Phase::Running, kept))) => {
                    (Some((Phase::Running, kept)), Queued::Kept(Phase::Running))
                }
                (false, Some((Phase::AtTurnEnd, kept))) => {
                    (Some((Phase::Running, kept)), Queued::Added(Phase::Running))
                }
                (false, None) => (
                    Some((Phase::Running, Plan::Resume)),
                    Queued::Added(Phase::Running),
                ),
            })
            .map_err(internal)?;
        match claimed {
            Queued::Ended => return self.revive_ended(id, Plan::Resume).await,
            Queued::Added(_) => {}
            Queued::Kept(_) | Queued::Refused => return Ok(Relaunched::Now),
        }
        let plan = self
            .graph
            .relaunch_plan(id)
            .map_err(internal)?
            .map_or(Plan::Resume, |(_, plan)| plan);
        let app = self.clone();
        tokio::spawn(async move { app.relaunch(node, plan, None).await })
            .await
            .map_err(internal)?
            .map_err(internal)?;
        Ok(Relaunched::Now)
    }

    pub(super) fn relaunch_stale(self: &Arc<Self>) -> rusqlite::Result<()> {
        for node in self.graph.active()? {
            let changed = node
                .rules
                .as_ref()
                .map(profile::changed_parts)
                .unwrap_or_default();
            if !changed.is_empty() && profile_of(&node).is_some() {
                let added = self.graph.plan_relaunch(node.id, |now| match now.pending {
                    None if !now.ended => (Some((Phase::AtTurnEnd, Plan::Resume)), true),
                    kept => (kept, false),
                })?;
                if added {
                    eprintln!(
                        "loom: 노드 {}의 시작 글이 바뀌었다({}). 턴 끝에 다시 띄운다",
                        node.id.0,
                        changed.join(", ")
                    );
                }
            }
            if self.at_turn_end(node.id) {
                self.server_turn_ended(node.id);
            }
        }
        Ok(())
    }

    pub(super) fn turn_ended(self: &Arc<Self>, node: NodeId) {
        self.run_at_turn_end(node, None);
    }

    pub(super) fn server_turn_ended(self: &Arc<Self>, node: NodeId) {
        let server = self.codex.lock().unwrap().contains_key(&node)
            || self.opencode2.lock().unwrap().contains_key(&node);
        if server && !self.busy(node) {
            self.turn_ended(node);
        }
    }

    pub(super) fn run_at_turn_end(self: &Arc<Self>, node: NodeId, guard: Option<Guard>) {
        let plan = self.change_plan(node, |now| match now.pending {
            Some((Phase::AtTurnEnd, plan)) => (Some((Phase::Running, plan.clone())), Some(plan)),
            kept => (kept, None),
        });
        let Some(plan) = plan else {
            return;
        };
        let app = self.clone();
        tokio::spawn(async move {
            let found = app.graph.active().map(|nodes| {
                nodes
                    .into_iter()
                    .find(|n| n.id == node)
                    .filter(|n| profile_of(n).is_some())
            });
            let relaunched = match found {
                Ok(Some(active)) => match app.relaunch(active, plan, guard).await {
                    Ok(Stop::Kept) => {
                        app.kept_at_turn_end(node);
                        return;
                    }
                    relaunched => relaunched.map(drop),
                },
                Ok(None) => {
                    app.drop_plan(node);
                    return;
                }
                Err(e) => {
                    app.drop_plan(node);
                    app.release_mail(node);
                    Err(e.to_string())
                }
            };
            if let Err(e) = relaunched {
                eprintln!("loom: 노드 {}를 턴 끝에 다시 띄우지 못했다: {e}", node.0);
            }
        });
    }

    pub(super) fn kept_running(&self, node: NodeId) {
        self.drop_plan(node);
        self.idle.release(node);
        self.graph.wake_mail();
        eprintln!(
            "loom: 노드 {}를 사람이 쓰고 있어 다시 띄우지 않는다",
            node.0
        );
    }

    pub(super) fn kept_at_turn_end(&self, node: NodeId) {
        self.change_plan(node, |now| {
            let next = now.pending.map(|(_, plan)| (Phase::AtTurnEnd, plan));
            (next, ())
        });
        eprintln!(
            "loom: 노드 {}를 사람이 쓰고 있어 다시 띄우기를 턴 끝으로 미룬다",
            node.0
        );
    }

    pub(super) async fn relaunch(
        self: &Arc<Self>,
        node: Active,
        plan: Plan,
        guard: Option<Guard>,
    ) -> Result<Stop, String> {
        let who = Who {
            id: Some(node.id),
            name: node.name.clone(),
        };
        let id = node.id;
        let switched = switch_of(&node, &plan);
        let result = match plan {
            Plan::Resume => self.restart(node, guard).await,
            Plan::Switch(to) => self.switch_agent(node, &to, guard).await,
        };
        if result == Ok(Stop::Kept) {
            return result;
        }
        self.drop_plan(id);
        if result.is_err() {
            self.release_mail(id);
        }
        self.record_relaunch(who, switched, &result.clone().map(drop));
        result
    }

    fn release_mail(&self, id: NodeId) {
        self.idle.end(id, IdleEnd::Relaunch);
        self.graph.wake_mail();
    }

    fn record_relaunch(
        &self,
        node: Who,
        switched: Option<AgentSwitch>,
        result: &Result<(), String>,
    ) {
        let entry = Entry::Relaunched {
            node,
            switched,
            error: result.as_ref().err().cloned(),
        };
        if let Err(e) = self.graph.record(&entry) {
            eprintln!("loom: 다시 띄운 결과를 남기지 못했다: {e}");
        }
    }

    async fn restart(self: &Arc<Self>, node: Active, guard: Option<Guard>) -> Result<Stop, String> {
        let (Some(agent), Some(profile)) = (node.agent.clone(), profile_of(&node)) else {
            return Err(format!(
                "노드 {}는 프로필이 있는 에이전트가 아니다",
                node.id.0
            ));
        };
        let settings = self.settings.get().map_err(|e| e.to_string())?;
        let place = self.graph.place_of(node.id).map_err(|e| e.to_string())?;
        let cwd = Path::new(&place.worktree);
        let resume = match profile {
            Profile::Claude | Profile::Codex => {
                self.carry_session(&node, profile, &settings, cwd).await?
            }
            Profile::Cursor => session_id(node.transcript.as_deref().map(Path::new))
                .ok_or("이어 갈 Cursor 세션 기록이 없다")?,
            Profile::Pi | Profile::Omp => pi_session(&node)?,
            Profile::OpenCode2 => match &node.server {
                Some(ServerLink::OpenCode2 { session, .. }) => session.clone(),
                _ => return Err("이어 갈 OpenCode 2 세션이 없다".into()),
            },
        };
        let brief = self.resumed_brief(&node, profile);
        let prepared = self
            .prepare_resumed(&node, &agent, profile, &resume, brief)
            .await?;
        if self.end_old(&node, prepared.discarder(), guard).await? == Stop::Kept {
            return Ok(Stop::Kept);
        }
        if profile == Profile::Claude {
            let _ = self.stop_claude_background(&node, &resume).await;
        }
        self.run_resumed(node.id, prepared, &resume, &settings, &agent)
            .await
            .map(|()| Stop::Stopped)
    }

    async fn switch_agent(
        self: &Arc<Self>,
        node: Active,
        to: &NewAgent,
        guard: Option<Guard>,
    ) -> Result<Stop, String> {
        let prepared = self.prepare_other(&node, to).await?;
        if guard
            .as_ref()
            .is_some_and(|keeps| !keeps(&self.host.screen(node.id)))
        {
            prepared.discarder()();
            return Ok(Stop::Kept);
        }
        if let Err(e) = self.stop_old_claude(&node).await {
            prepared.discarder()();
            return Err(e);
        }
        if self.end_old(&node, prepared.discarder(), guard).await? == Stop::Kept {
            return Ok(Stop::Kept);
        }
        self.run_other(node.id, prepared, to)
            .await
            .map(|()| Stop::Stopped)
    }

    pub(super) async fn stop_old_claude(&self, node: &Active) -> Result<(), String> {
        match session_id(node.transcript.as_deref().map(Path::new)) {
            Some(session) if profile_of(node) == Some(Profile::Claude) => {
                self.stop_claude_background(node, &session).await
            }
            _ => Ok(()),
        }
    }

    pub(super) async fn end_old(
        &self,
        node: &Active,
        discard: impl Fn(),
        guard: Option<Guard>,
    ) -> Result<Stop, String> {
        let (done, exited) = oneshot::channel();
        self.relaunches
            .exiting
            .lock()
            .unwrap()
            .insert(node.id, done);
        let snapshot = Snapshot::now();
        let shells = self.background_shells(&snapshot, node);
        match guard {
            None => {
                self.feed(node.id, AgentEvent::RelaunchStarted);
                snapshot.kill_trees(shells);
                self.idle.end(node.id, IdleEnd::Relaunch);
                self.host.stop(node.id);
            }
            Some(keeps) => {
                if !self.host.stop_if(node.id, |keys| keeps(&keys.screen())) {
                    self.relaunches.exiting.lock().unwrap().remove(&node.id);
                    discard();
                    return Ok(Stop::Kept);
                }
                self.feed(node.id, AgentEvent::RelaunchStarted);
                snapshot.kill_trees(shells);
                self.idle.end(node.id, IdleEnd::Relaunch);
            }
        }
        if tokio::time::timeout(EXIT_WAIT, exited).await.is_err() {
            self.relaunches.exiting.lock().unwrap().remove(&node.id);
            self.feed(node.id, AgentEvent::RelaunchFailed);
            discard();
            return Err(format!("노드 {}의 옛 프로세스가 끝나지 않았다", node.id.0));
        }
        Ok(Stop::Stopped)
    }

    async fn prepare_other(&self, node: &Active, to: &NewAgent) -> Result<super::Prepared, String> {
        let from = node.agent.as_ref().ok_or("에이전트 노드가 아니다")?;
        let record = self.old_record(node).await;
        let start = Start::Agent {
            agent: to.agent.clone(),
            brief: handoff(from, node.role.as_deref(), record.as_deref()),
            choice: to.choice.clone(),
            resume: None,
        };
        self.prepare_start(node.id, &start).await
    }

    async fn old_record(&self, node: &Active) -> Option<String> {
        match profile_of(node)? {
            Profile::Claude | Profile::Cursor | Profile::Pi | Profile::Omp => {
                node.transcript.clone()
            }
            Profile::Codex => {
                let server = self.codex.lock().unwrap().get(&node.id).cloned()?;
                let thread = server.thread().borrow().as_ref().map(|t| t.id.clone())?;
                server.rollout(&thread).await.ok().flatten()
            }
            Profile::OpenCode2 => None,
        }
    }

    async fn run_other(
        self: &Arc<Self>,
        id: NodeId,
        prepared: super::Prepared,
        to: &NewAgent,
    ) -> Result<(), String> {
        let discard = prepared.discarder();
        let model = Model {
            model: to.choice.model.clone(),
            effort: to.choice.effort.clone(),
        };
        if let Err(e) = self
            .graph
            .switched(id, &prepared.launched(), &to.agent, &model)
        {
            discard();
            let _ = self.graph.lost(id);
            return Err(e.to_string());
        }
        self.codex.lock().unwrap().remove(&id);
        self.opencode2.lock().unwrap().remove(&id);
        self.drop_plan(id);
        if let Err(e) = self.launch(id, prepared) {
            discard();
            let _ = self.graph.lost(id);
            return Err(e);
        }
        if let Err(e) = self.graph.limit_cleared(id) {
            eprintln!("loom: 노드 {}의 한도 표시를 지우지 못했다: {e}", id.0);
        }
        Ok(())
    }

    pub(super) async fn revive(self: &Arc<Self>, node: Active, notice: &str) -> Result<(), String> {
        let (Some(agent), Some(profile)) = (node.agent.clone(), profile_of(&node)) else {
            return Err("프로필이 있는 에이전트가 아니다".into());
        };
        let settings = self.settings.get().map_err(|e| e.to_string())?;
        let place = self.graph.place_of(node.id).map_err(|e| e.to_string())?;
        let resume = match (profile, &node.server) {
            (Profile::Claude, _) => {
                self.carry_session(&node, profile, &settings, Path::new(&place.worktree))
                    .await?
            }
            (Profile::Cursor, _) => session_id(node.transcript.as_deref().map(Path::new))
                .ok_or("이어 갈 Cursor 세션 기록이 없다")?,
            (Profile::Pi | Profile::Omp, _) => pi_session(&node)?,
            (
                Profile::Codex,
                Some(ServerLink::Codex {
                    thread: Some(thread),
                    ..
                }),
            ) => thread.clone(),
            (Profile::OpenCode2, Some(ServerLink::OpenCode2 { session, .. })) => session.clone(),
            (Profile::Codex | Profile::OpenCode2, _) => {
                return Err("이어 갈 서버 세션이 없다".into());
            }
        };
        let prepared = self
            .prepare_resumed(&node, &agent, profile, &resume, notice.to_owned())
            .await?;
        if profile == Profile::Claude {
            let _ = self.stop_claude_background(&node, &resume).await;
        }
        self.run_resumed(node.id, prepared, &resume, &settings, &agent)
            .await
    }

    async fn prepare_resumed(
        &self,
        node: &Active,
        agent: &AgentId,
        profile: Profile,
        resume: &str,
        brief: String,
    ) -> Result<super::Prepared, String> {
        let start = Start::Agent {
            agent: agent.clone(),
            brief,
            choice: match profile {
                Profile::OpenCode2 => ModelChoice::default(),
                Profile::Claude | Profile::Codex | Profile::Cursor | Profile::Pi | Profile::Omp => {
                    ModelChoice {
                        model: node.model.model.clone(),
                        effort: node.model.effort.clone(),
                    }
                }
            },
            resume: Some(resume.to_owned()),
        };
        self.prepare_start(node.id, &start).await
    }

    async fn prepare_start(&self, id: NodeId, start: &Start) -> Result<super::Prepared, String> {
        let place = self.graph.place_of(id).map_err(|e| e.to_string())?;
        let position = match self.graph.relations(id) {
            Ok(r) if r.parent.is_some() => Position::Child,
            Ok(_) => Position::Root,
            Err(e) => return Err(e.to_string()),
        };
        let task = self.task_dir(id).await.map_err(|e| format!("{e:?}"))?;
        self.prepare(start, Path::new(&place.worktree), position, Some(&task))
            .await
            .map_err(|e| format!("{e:?}"))
    }

    async fn run_resumed(
        self: &Arc<Self>,
        id: NodeId,
        prepared: super::Prepared,
        resume: &str,
        settings: &SettingsView,
        agent: &AgentId,
    ) -> Result<(), String> {
        let discard = prepared.discarder();
        if let Some(server) = &prepared.codex {
            let bypass = !settings.bypass_off.contains(agent);
            let call = |method, params| server.call(method, params);
            if let Err(e) = codex::resume(call, resume, bypass).await {
                discard();
                let _ = self.graph.lost(id);
                return Err(e);
            }
            server.follow_thread(resume.to_owned()).await;
        }
        if let Err(e) = self.graph.relaunched(id, &prepared.launched()) {
            discard();
            let _ = self.graph.lost(id);
            return Err(e.to_string());
        }
        self.drop_plan(id);
        if let Err(e) = self.launch(id, prepared) {
            discard();
            let _ = self.graph.lost(id);
            return Err(e);
        }
        if let Err(e) = self.graph.limit_cleared(id) {
            eprintln!("loom: 노드 {}의 한도 표시를 지우지 못했다: {e}", id.0);
        }
        Ok(())
    }

    async fn carry_session(
        &self,
        node: &Active,
        profile: Profile,
        settings: &SettingsView,
        cwd: &Path,
    ) -> Result<String, String> {
        let provider = profile.provider().ok_or("계정이 없는 프로필이다")?;
        let site = self.site_at(cwd).await.map_err(|e| format!("{e:?}"))?;
        let target = settings.account(provider, site.distro()).cloned();
        let new_root = site
            .login_dir(provider, target.as_ref())
            .ok_or("홈 폴더를 찾지 못했다")?;
        if profile == Profile::Codex {
            return self.copy_codex_thread(node.id, &site, &new_root).await;
        }
        let transcript = node.transcript.as_deref().map(|t| site.local(t));
        let session = copy_claude_session(transcript.as_deref(), &new_root)?;
        if let (Some(to), Some(from)) = (
            site.accounts().config_dir(provider, target.as_ref()),
            site.claude_files(node.account.as_ref()),
        ) {
            accounts::carry_claude_consent(&from, &to, &site.cli_cwd(cwd))
                .map_err(|e| format!("{}: {e}", to.display()))?;
        }
        Ok(session)
    }

    async fn stop_claude_background(&self, node: &Active, session: &str) -> Result<(), String> {
        let Ok(place) = self.graph.place_of(node.id) else {
            return Ok(());
        };
        let Ok(site) = self.site_at(Path::new(&place.worktree)).await else {
            return Ok(());
        };
        let account = node.account.as_ref();
        let Ok(list) = site.command(Provider::Claude, &["agents", "--json"], account) else {
            return Ok(());
        };
        let Some(out) = run_claude(list).await else {
            return Ok(());
        };
        let Some(job) = background_job(&out, session) else {
            return Ok(());
        };
        let stop = site
            .command(Provider::Claude, &["stop", &job], account)
            .map_err(|e| e.to_string())?;
        run_claude(stop).await.map(drop).ok_or_else(|| {
            format!("`claude stop {job}`이 옛 세션의 백그라운드 작업을 끝내지 못했다")
        })
    }

    fn resumed_brief(&self, node: &Active, profile: Profile) -> String {
        let limited = self.graph.limited(node.id).unwrap_or_else(|e| {
            eprintln!("loom: 노드 {}의 한도 표시를 읽지 못했다: {e}", node.id.0);
            false
        });
        let stopped = self.background_commands(&Snapshot::now(), node);
        let cut = self.agent_state(node.id).is_some_and(|s| s.cut());
        let unread = self.graph.unread_mail(node.id).unwrap_or_else(|e| {
            eprintln!("loom: 노드 {}의 메일함을 읽지 못했다: {e}", node.id.0);
            true
        });
        let opens_no_wait = matches!(profile, Profile::Cursor | Profile::Pi | Profile::Omp);
        let first = match limited {
            true => Some(CONTINUE_PROMPT),
            false => (cut || unread || !stopped.is_empty() || opens_no_wait)
                .then_some(profile::RESUMED_PROMPT),
        };
        let stopped = (!stopped.is_empty()).then(|| stopped_notice(&stopped));
        first
            .map(str::to_owned)
            .into_iter()
            .chain(stopped)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn background_commands(&self, snapshot: &Snapshot, node: &Active) -> Vec<String> {
        self.background_shells(snapshot, node)
            .into_iter()
            .map(|shell| snapshot.command_lines(shell))
            .filter(|lines| !lines.first().is_some_and(|shell| runs_hook(shell)))
            .filter_map(|lines| lines.get(1).or(lines.first()).map(|line| shown(line)))
            .collect()
    }

    pub(super) fn hook_running(&self, node: NodeId) -> bool {
        self.host.pid(node).is_some_and(|root| {
            Snapshot::now()
                .command_lines(root)
                .iter()
                .any(|line| runs_hook(line))
        })
    }

    pub(super) fn background_shells(&self, snapshot: &Snapshot, node: &Active) -> Vec<u32> {
        let server = match &node.server {
            Some(ServerLink::Codex { pid, .. } | ServerLink::OpenCode2 { pid, .. }) => *pid,
            None => None,
        };
        [self.host.pid(node.id), server]
            .into_iter()
            .flatten()
            .flat_map(|root| snapshot.session_leaders_under(root))
            .collect()
    }

    async fn copy_codex_thread(
        &self,
        node: NodeId,
        site: &Site,
        new_root: &Path,
    ) -> Result<String, String> {
        let server = self.codex.lock().unwrap().get(&node).cloned();
        let thread = server
            .as_ref()
            .and_then(|s| s.thread().borrow().as_ref().map(|t| t.id.clone()));
        let (Some(server), Some(thread)) = (server, thread) else {
            return Err(format!("노드 {}에는 이어 갈 Codex 스레드가 없다", node.0));
        };
        let rollout = server
            .rollout(&thread)
            .await?
            .ok_or_else(|| format!("스레드 {thread}의 기록 파일이 없다"))?;
        copy_session(&site.local(&rollout), CODEX_ROLLOUT_DEPTH, new_root)?;
        Ok(thread)
    }
}

enum Queued {
    Ended,
    Refused,
    Kept(Phase),
    Added(Phase),
}

pub(super) fn profile_of(node: &Active) -> Option<Profile> {
    node.agent.as_ref().and_then(profile::profile_of)
}

pub(super) fn provider_of(node: &Active) -> Option<Provider> {
    profile_of(node).and_then(Profile::provider)
}

fn switch_of(node: &Active, plan: &Plan) -> Option<AgentSwitch> {
    match plan {
        Plan::Resume => None,
        Plan::Switch(to) => Some(AgentSwitch {
            from: node.agent.clone()?,
            to: to.agent.clone(),
        }),
    }
}

fn handoff(from: &AgentId, role: Option<&str>, record: Option<&str>) -> String {
    let role = role.map_or(String::new(), |r| format!(" Your role is {r}."));
    let record = match record {
        Some(path) => format!(
            "The old session record is {path}. Its first user message is your brief. Read the record to learn the task and where agent {} stopped.",
            from.0
        ),
        None => "loom does not know the old session record. Ask your parent (or the human, if you have no parent) for your brief.".to_owned(),
    };
    format!(
        "loom replaced agent {} in this node with you. A session cannot move between agents, so you start a new session. The node keeps its ID, name, parent, children, mailbox, and worktree.{role}\n{record}\nThen run `git status`, look at the worktree, and continue the task.",
        from.0
    )
}

fn stopped_notice(commands: &[String]) -> String {
    let list: String = commands.iter().map(|c| format!("\n- {c}")).collect();
    format!(
        "loom relaunched your process and stopped these background commands that you started before:{list}\nStart them again if you still need them."
    )
}

fn runs_hook(shell: &str) -> bool {
    let line = shell.trim_end_matches('"');
    HOOK_COMMANDS.iter().any(|hook| line.ends_with(hook))
}

fn shown(line: &str) -> String {
    match line.char_indices().nth(STOPPED_LINE_CHARS) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line.to_owned(),
    }
}

fn pi_session(node: &Active) -> Result<String, &'static str> {
    node.transcript
        .clone()
        .ok_or("이어 갈 Pi·OMP 세션 기록이 없다")
}

fn session_id(transcript: Option<&Path>) -> Option<String> {
    transcript?
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
}

fn copy_claude_session(transcript: Option<&Path>, new_root: &Path) -> Result<String, String> {
    let file = transcript
        .filter(|f| f.is_file())
        .ok_or("이어 갈 Claude 세션 기록이 없다")?;
    let session = session_id(Some(file)).ok_or("세션 기록 파일 이름이 없다")?;
    copy_session(file, CLAUDE_TRANSCRIPT_DEPTH, new_root)?;
    let side = file.with_extension("");
    if side.is_dir()
        && let Some(to) = relocate(&side, CLAUDE_TRANSCRIPT_DEPTH, new_root)
        && to != side
    {
        copy_dir(&side, &to).map_err(|e| format!("{}: {e}", side.display()))?;
    }
    Ok(session)
}

fn copy_session(file: &Path, depth: usize, new_root: &Path) -> Result<(), String> {
    let to = relocate(file, depth, new_root)
        .ok_or_else(|| format!("{}: 로그인 폴더 아래 기록 파일이 아니다", file.display()))?;
    if to == file {
        return Ok(());
    }
    let copied = to
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| copy_file(file, &to));
    copied
        .map(drop)
        .map_err(|e| format!("{} → {}: {e}", file.display(), to.display()))
}

fn relocate(file: &Path, depth: usize, new_root: &Path) -> Option<PathBuf> {
    let parts: Vec<_> = file.components().collect();
    let tail = parts.get(parts.len().checked_sub(depth)?..)?;
    Some(tail.iter().fold(new_root.to_path_buf(), |p, c| p.join(c)))
}

fn copy_file(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::write(to, std::fs::read(from)?)
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            copy_file(&entry.path(), &target)?;
        }
    }
    Ok(())
}

async fn run_claude(cmd: std::process::Command) -> Option<Vec<u8>> {
    let mut cmd = tokio::process::Command::from(cmd);
    cmd.stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(CLAUDE_CLI_WAIT, cmd.output())
        .await
        .ok()?
        .ok()?;
    out.status.success().then_some(out.stdout)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeJob {
    id: Option<String>,
    kind: String,
    session_id: String,
}

fn background_job(json: &[u8], session: &str) -> Option<String> {
    let jobs: Vec<ClaudeJob> = serde_json::from_slice(json).ok()?;
    jobs.into_iter()
        .find(|j| j.kind == "background" && j.session_id == session)?
        .id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Launched, Place};
    use crate::worktree::RepoId;

    fn app() -> (tempfile::TempDir, Arc<App>) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("loom.db");
        let database = crate::db::open(&db_path).unwrap();
        database
            .lock()
            .execute("INSERT INTO repos (id, path) VALUES (1, '/repo')", [])
            .unwrap();
        let addr = "127.0.0.1:0".parse().unwrap();
        let app = Arc::new(
            App::new(
                database,
                &db_path,
                addr,
                None,
                None,
                tokio::sync::mpsc::unbounded_channel().0,
            )
            .unwrap(),
        );
        (dir, app)
    }

    fn stale_pi(app: &App, token: &str) -> NodeId {
        let place = Place {
            repo: RepoId(1),
            worktree: "/repo".into(),
        };
        let launched = Launched {
            token: token.into(),
            server: None,
            account: None,
            rules: Some([("usage/pi".to_owned(), "old".to_owned())].into()),
        };
        app.graph
            .add_root(&place, Some(AgentId("pi".into())), None, &launched, |_| {
                Ok(())
            })
            .unwrap()
    }

    fn relaunch_errors(app: &App) -> Vec<Option<String>> {
        app.graph
            .recent_events()
            .unwrap()
            .into_iter()
            .filter_map(|r| match r.entry {
                Entry::Relaunched { error, .. } => Some(error),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn a_stale_node_waits_for_its_turn_end_until_its_process_ends() {
        let (_dir, app) = app();
        let live = stale_pi(&app, "live");
        let ended = stale_pi(&app, "ended");
        app.graph.exited(ended, 0).unwrap();
        app.relaunch_stale().unwrap();
        assert!(app.at_turn_end(live));
        assert!(!app.holds_mail(ended), "no turn end can come");
        app.hold(ended);
        app.process_ended(ended);
        assert!(app.holds_mail(ended), "a running relaunch stays");
        app.graph.exited(live, 0).unwrap();
        app.process_ended(live);
        assert!(!app.holds_mail(live), "the exit clears the plan");
    }

    #[tokio::test]
    async fn a_relaunch_after_the_process_ended_starts_it_or_reports_why_not() {
        let (_dir, app) = app();
        let node = stale_pi(&app, "gone");
        app.relaunch_stale().unwrap();
        let before = app.graph.active().unwrap().pop().unwrap();
        app.graph.exited(node, 0).unwrap();
        let queued = app.queue(before, Plan::Resume).await;
        assert!(queued.is_err(), "{queued:?}");
        assert_eq!(
            relaunch_errors(&app),
            [Some("이어 갈 Pi·OMP 세션 기록이 없다".to_owned())]
        );
        assert!(!app.holds_mail(node));
        app.change_plan(node, |_| (Some((Phase::AtTurnEnd, Plan::Resume)), ()));
        let revived = app.relaunch_by_id(node, Plan::Resume).await;
        assert!(revived.is_err(), "{revived:?}");
        assert_eq!(relaunch_errors(&app).len(), 2);
        assert!(!app.holds_mail(node));
    }

    #[tokio::test]
    async fn an_endpoint_stream_serves_only_the_token_of_its_own_node() {
        use axum::extract::ConnectInfo;
        use tower::ServiceExt;
        let (_dir, app) = app();
        let mine = stale_pi(&app, "mine");
        stale_pi(&app, "other");
        let router = Router::new()
            .route("/api/agent/status", axum::routing::post(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                app.clone(),
                super::super::runtime::from_its_node,
            ));
        let status = |token: &'static str| {
            let request = axum::http::Request::post("/api/agent/status")
                .header("authorization", format!("Bearer {token}"))
                .extension(ConnectInfo(mine))
                .body(axum::body::Body::empty())
                .unwrap();
            let router = router.clone();
            async move { router.oneshot(request).await.unwrap().status() }
        };
        assert_eq!(status("mine").await, axum::http::StatusCode::OK);
        assert_eq!(status("other").await, axum::http::StatusCode::FORBIDDEN);
        assert_eq!(status("unknown").await, axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn a_failed_node_endpoint_is_in_the_event_log_but_not_in_the_console() {
        use crate::graph::Entry;
        use crate::node_host::Linked;
        let (_dir, app) = app();
        let node = stale_pi(&app, "tok");
        let (links, linked) = tokio::sync::mpsc::unbounded_channel();
        let (doors, _door) = crate::tunnel::Door::new();
        links
            .send((node, Linked::EndpointFailed("accept: no fd".into())))
            .unwrap();
        drop(links);
        app.clone().carry_links(linked, doors).await;
        let recorded = app.graph.recent_events().unwrap();
        assert!(recorded.iter().any(|r| matches!(
            &r.entry,
            Entry::EndpointFailed { node: who, error } if who.id == Some(node) && error == "accept: no fd"
        )));
        assert!(
            app.graph
                .event_log()
                .unwrap()
                .records
                .iter()
                .all(|r| !matches!(r.entry, Entry::EndpointFailed { .. }))
        );
    }

    #[tokio::test]
    async fn a_node_call_in_an_unknown_protocol_is_refused_and_relaunches_the_node() {
        use crate::protocol::{NODE_CALLS, NODE_CALLS_HEADER};
        let (_dir, app) = app();
        let node = stale_pi(&app, "tok");
        let headers = |protocol: Option<String>| {
            let mut headers = HeaderMap::new();
            headers.insert("authorization", "Bearer tok".parse().unwrap());
            if let Some(protocol) = protocol {
                headers.insert(NODE_CALLS_HEADER, protocol.parse().unwrap());
            }
            headers
        };
        assert_eq!(app.caller(&headers(None)).ok(), Some(node), "an old hook");
        assert_eq!(
            app.caller(&headers(Some(NODE_CALLS.now.to_string()))).ok(),
            Some(node)
        );
        assert_eq!(
            app.graph.agent_state(node).unwrap(),
            Some(AgentState::Starting)
        );
        let newer = (NODE_CALLS.now + 1).to_string();
        let Err((status, text)) = app.caller(&headers(Some(newer.clone()))) else {
            panic!("a newer protocol is refused");
        };
        assert_eq!(status, axum::http::StatusCode::UPGRADE_REQUIRED);
        assert!(text.contains(&format!("node protocol {newer}")), "{text}");
        for _ in 0..500 {
            if !relaunch_errors(&app).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            relaunch_errors(&app),
            [Some("이어 갈 Pi·OMP 세션 기록이 없다".to_owned())],
            "the refusal relaunches the node"
        );
        assert!(app.graph.recent_events().unwrap().iter().any(|r| matches!(
            &r.entry,
            Entry::AgentStateChanged {
                by: AgentEvent::VersionMismatch,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn an_unserved_hook_relaunches_a_node_in_its_turn_now() {
        let (_dir, app) = app();
        let node = stale_pi(&app, "busy");
        app.idle.turn_started(node, "prompt".into());
        assert!(app.busy(node));
        let active = || app.graph.active().unwrap().pop().unwrap();
        assert_eq!(
            app.queue(active(), Plan::Resume).await.ok(),
            Some(Relaunched::AtTurnEnd)
        );
        assert!(relaunch_errors(&app).is_empty());
        let now = app.relaunch_unserved(node).await;
        assert!(now.is_err(), "{now:?}");
        assert_eq!(
            relaunch_errors(&app),
            [Some("이어 갈 Pi·OMP 세션 기록이 없다".to_owned())]
        );
        assert!(!app.holds_mail(node));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_first_input_of_a_relaunch_follows_the_old_state_the_mail_and_the_stopped_commands()
    {
        let (dir, app) = app();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        crate::fake_exe::write_executable(&bin.join("loom"), "#!/bin/sh\nsleep 30\n");
        let place = Place {
            repo: RepoId(1),
            worktree: "/repo".into(),
        };
        let launched = Launched {
            token: "t".into(),
            server: None,
            account: None,
            rules: None,
        };
        let id = app
            .graph
            .add_root(
                &place,
                Some(AgentId("claude".into())),
                None,
                &launched,
                |_| Ok(()),
            )
            .unwrap();
        let mut cmd = portable_pty::CommandBuilder::new("sh");
        cmd.args([
            "-c",
            "setsid sleep 37 & PATH=\"$BIN:$PATH\" setsid sh -c 'loom hook' & exec sleep 60",
        ]);
        cmd.env("BIN", &bin);
        crate::node_host::in_process(&dir.path().join("pty").join("nodes"), id, cmd);
        app.host.resume(id, |_| {}).unwrap();
        let transcript = dir.path().join("s.jsonl");
        std::fs::write(
            &transcript,
            "{\"type\":\"assistant\"}\n{\"type\":\"system\",\"subtype\":\"turn_duration\"}\n",
        )
        .unwrap();
        app.graph
            .set_transcript(id, transcript.to_str().unwrap())
            .unwrap();
        let node = app.graph.active().unwrap().pop().unwrap();
        for _ in 0..250 {
            if app.background_shells(&Snapshot::now(), &node).len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            app.background_commands(&Snapshot::now(), &node),
            ["sleep 37"]
        );
        let stopped = "loom relaunched your process and stopped these background commands that you started before:\n- sleep 37\nStart them again if you still need them.";
        app.feed(id, AgentEvent::WaitOpened);
        assert_eq!(
            app.resumed_brief(&node, Profile::Claude),
            format!("{}\n{stopped}", profile::RESUMED_PROMPT)
        );
        let snapshot = Snapshot::now();
        snapshot.kill_trees(app.background_shells(&snapshot, &node));
        while !app.background_commands(&Snapshot::now(), &node).is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let brief = |profile| app.resumed_brief(&node, profile);
        assert_eq!(brief(Profile::Claude), "", "a listening node without mail");
        assert_eq!(brief(Profile::Cursor), profile::RESUMED_PROMPT);
        app.feed(id, AgentEvent::WaitClosed { by: IdleEnd::Gone });
        assert_eq!(brief(Profile::Claude), profile::RESUMED_PROMPT, "deaf");
        app.feed(id, AgentEvent::TurnStarted);
        assert_eq!(
            brief(Profile::Claude),
            profile::RESUMED_PROMPT,
            "a cut turn"
        );
        app.feed(id, AgentEvent::WaitOpened);
        let sender = app
            .graph
            .add_root(
                &place,
                None,
                None,
                &Launched {
                    token: "s".into(),
                    ..launched
                },
                |_| Ok(()),
            )
            .unwrap();
        app.graph
            .note(sender, &format!("#{}", id.0), Kind::Queue, "일", None)
            .unwrap();
        assert_eq!(
            brief(Profile::Claude),
            profile::RESUMED_PROMPT,
            "unread mail"
        );
        app.host.kill(id);
    }

    #[test]
    fn only_a_background_job_of_the_same_full_session_is_stopped() {
        let json = br#"[
            {"pid":1,"id":"925fee83","kind":"background","sessionId":"925fee83-0000-4000-8000-000000000002","status":"idle"},
            {"pid":2,"kind":"interactive","sessionId":"aaaaaaaa-0000-4000-8000-000000000001"}
        ]"#;
        let mine = "aaaaaaaa-0000-4000-8000-000000000001";
        assert_eq!(
            background_job(json, "925fee83-ffff-4000-8000-00000000000f"),
            None
        );
        assert_eq!(background_job(json, mine), None);
        assert_eq!(
            background_job(json, "925fee83-0000-4000-8000-000000000002").as_deref(),
            Some("925fee83")
        );
        assert_eq!(background_job(b"not json", mine), None);
        assert_eq!(background_job(b"[]", mine), None);
    }

    #[test]
    fn handoff_names_the_old_agent_role_and_record() {
        let claude = AgentId("claude".into());
        let text = handoff(
            &claude,
            Some("코디"),
            Some("/h/.claude/projects/-w/s1.jsonl"),
        );
        assert!(text.contains("replaced agent claude"), "{text}");
        assert!(text.contains("Your role is 코디."), "{text}");
        assert!(
            text.contains("/h/.claude/projects/-w/s1.jsonl. Its first user message is your brief"),
            "{text}"
        );
        assert!(text.contains("`git status`"), "{text}");
        let unknown = handoff(&claude, None, None);
        assert!(!unknown.contains("role"), "{unknown}");
        assert!(unknown.contains("Ask your parent"), "{unknown}");
    }

    #[test]
    fn session_files_keep_their_path_under_the_new_login_folder() {
        assert_eq!(
            relocate(
                Path::new("/h/.claude/projects/-w/abc.jsonl"),
                CLAUDE_TRANSCRIPT_DEPTH,
                Path::new("/l/acc")
            ),
            Some(PathBuf::from("/l/acc/projects/-w/abc.jsonl"))
        );
        assert_eq!(
            relocate(
                Path::new("/h/.codex/sessions/2026/10/21/rollout-x-t1.jsonl"),
                CODEX_ROLLOUT_DEPTH,
                Path::new("/l/c")
            ),
            Some(PathBuf::from("/l/c/sessions/2026/10/21/rollout-x-t1.jsonl"))
        );
        assert_eq!(relocate(Path::new("a.jsonl"), 3, Path::new("/l")), None);
    }

    #[test]
    fn session_id_is_the_transcript_file_name() {
        let cursor =
            Path::new("/h/.cursor/projects/tmp-w/agent-transcripts/93f2-8533/93f2-8533.jsonl");
        assert_eq!(session_id(Some(cursor)).as_deref(), Some("93f2-8533"));
        assert_eq!(session_id(None), None);
    }

    #[test]
    fn claude_session_copies_the_transcript_and_its_side_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("old/projects/-w");
        std::fs::create_dir_all(old.join("s1/subagents")).unwrap();
        std::fs::write(old.join("s1.jsonl"), "x").unwrap();
        std::fs::write(old.join("s1/subagents/a.jsonl"), "y").unwrap();
        let new = tmp.path().join("new");
        let transcript = old.join("s1.jsonl");
        assert_eq!(copy_claude_session(Some(&transcript), &new).unwrap(), "s1");
        assert_eq!(
            std::fs::read_to_string(new.join("projects/-w/s1.jsonl")).unwrap(),
            "x"
        );
        assert_eq!(
            std::fs::read_to_string(new.join("projects/-w/s1/subagents/a.jsonl")).unwrap(),
            "y"
        );
    }

    #[test]
    fn no_stored_session_fails_and_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let new = tmp.path().join("new");
        assert!(copy_claude_session(None, &new).is_err());
        let missing = tmp.path().join("old/projects/-w/gone.jsonl");
        assert!(copy_claude_session(Some(&missing), &new).is_err());
        assert!(!new.exists());
    }
}
