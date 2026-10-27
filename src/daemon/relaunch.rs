use std::path::Path;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path as UrlPath, State};
use axum::http::HeaderMap;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{AgentError, App, Problem, RelaunchRequest, for_agent, internal};
use crate::accounts::Site;
use crate::graph::{
    Active, AgentEvent, AgentState, AgentSwitch, Entry, IdleEnd, NewAgent, Next, NodeId, Phase,
    Plan, ProcessOutcome, Who,
};
use crate::process_tree::Snapshot;
use crate::profile::adapter::{Carry, ResumeError};
use crate::profile::process::Served;
use crate::profile::{self, ModelRule, Profile, Start};
use crate::protocol::{AgentId, Model, ModelChoice, Provider, Refusal, Session};
use crate::screen::Screen;
use crate::settings::SettingsView;

pub(super) const EXIT_WAIT: Duration = Duration::from_secs(10);
const CONTINUE_PROMPT: &str =
    "Your account was switched after a usage limit. Continue the task where you stopped.";
const WOKEN_NOTICE: &str = "loom ended your process while you were done and idle, to free memory. It started your process again in the same session. \
     Messages that came for you arrive next.";
const REVIVED_NOTICE: &str = "Your process ended, and loom started it again in the same session. \
     Messages sent while it was ended did not arrive. Run `loom status`, then continue your work.";
const HOST_GONE_NOTICE: &str = "Your host stopped (for example a WSL shutdown or a reboot), and loom started your process again in the same session. \
     Messages that came for you arrive next. If your last turn was cut, continue it. Otherwise end your turn.";
pub(super) const FINISH_TURN: &str = "loom relaunches you when this turn ends. If you have no work in progress, end this turn now; otherwise continue.";
const STOPPED_LINE_CHARS: usize = 200;

pub(super) type Guard = Arc<dyn Fn(&Screen) -> bool + Send + Sync>;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Stop {
    Stopped,
    Kept,
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
    Router::new().route("/api/nodes/{id}/relaunch", post(relaunch_node))
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
        Some(name) => app.graph.any_node(caller, name)?,
    };
    let plan = req.to.map_or(Plan::Resume, Plan::Switch);
    app.relaunch_by_id(node, plan).await
}

impl App {
    pub(super) fn relaunch_planned(&self, node: NodeId) -> bool {
        self.pending(node).is_some()
    }

    pub(super) fn relaunching(&self, node: NodeId) -> bool {
        self.pending(node).is_some_and(Phase::started)
    }

    pub(super) fn awaits_turn_end(&self, node: NodeId) -> bool {
        self.pending(node) == Some(Phase::AtTurnEnd) && self.busy(node)
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

    fn drop_plan(&self, node: NodeId) {
        plan_written(node, self.graph.drop_relaunch(node));
    }

    fn finish_plan(&self, node: NodeId) {
        plan_written(node, self.graph.finish_relaunch(node));
    }

    pub(super) fn process_ended(&self, node: NodeId) {
        if plan_written(node, self.graph.drop_turn_end(node)) {
            eprintln!(
                "loom: 노드 {}의 프로세스가 끝나 턴 끝 다시 띄우기를 지운다",
                node.0
            );
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
            return Err(profile::no_link(agent).into());
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
        let ended = self.graph.process(id).map_err(internal)?;
        let asleep = ended == Some(ProcessOutcome::Asleep);
        let waiting = asleep || ended == Some(ProcessOutcome::HostGone);
        if profile_of(&node).is_none() {
            if waiting {
                self.graph.sleep_lost(id).map_err(internal)?;
            }
            return Err(Refusal::NoResume(node.name).into());
        }
        let plan = self.chosen(&node, plan)?;
        let notice = match ended {
            Some(ProcessOutcome::Asleep) => WOKEN_NOTICE,
            Some(ProcessOutcome::HostGone) => HOST_GONE_NOTICE,
            _ => REVIVED_NOTICE,
        };
        let plan = match self.graph.claim_revive(id, &plan).map_err(internal)? {
            Next::Start(plan) => plan,
            Next::Refuse => return Err(Refusal::Relaunching(node.name).into()),
            Next::Keep | Next::Wait | Next::ReviveEnded => return Ok(Relaunched::Now),
        };
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
                    if let Err(e) = app.end_detached(&node).await {
                        prepared.discarder()();
                        return Err(e.into());
                    }
                    Ok(app.run_other(node.id, prepared, &to).await?)
                }
            }
        })
        .await
        .unwrap_or_else(|e| Err(ResumeError::Failed(e.to_string())));
        self.finish_plan(id);
        let kept = match &result {
            Err(ResumeError::NoSession(_)) if waiting => self.graph.sleep_lost(id),
            Err(ResumeError::Failed(_)) if asleep => self.graph.sleep_again(id),
            _ => Ok(()),
        };
        if let Err(e) = kept {
            eprintln!("loom: 깨우지 못한 노드 {}를 적지 못했다: {e}", id.0);
        }
        let result = result.map_err(|e| e.to_string());
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
        match self
            .graph
            .request_relaunch(node.id, &plan)
            .map_err(internal)?
        {
            Next::ReviveEnded => self.revive_ended(node.id, plan).await,
            Next::Refuse => Err(Refusal::Relaunching(node.name).into()),
            Next::Wait => match self.run_unless_busy(node.id) {
                true => Ok(Relaunched::Now),
                false => Ok(Relaunched::AtTurnEnd),
            },
            Next::Start(plan) => self.relaunch_now(node, plan).await,
            Next::Keep => Ok(Relaunched::Now),
        }
    }

    async fn relaunch_now(
        self: &Arc<Self>,
        node: Active,
        plan: Plan,
    ) -> Result<Relaunched, Problem> {
        let app = self.clone();
        tokio::spawn(async move { app.relaunch(node, plan, None).await })
            .await
            .map_err(internal)?
            .map_err(internal)?;
        Ok(Relaunched::Now)
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
        match self.graph.claim_unserved(id).map_err(internal)? {
            Next::ReviveEnded => self.revive_ended(id, Plan::Resume).await,
            Next::Start(plan) => self.relaunch_now(node, plan).await,
            Next::Keep | Next::Wait | Next::Refuse => Ok(Relaunched::Now),
        }
    }

    pub(super) fn relaunch_stale(self: &Arc<Self>) -> rusqlite::Result<()> {
        for node in self.graph.active()? {
            let changed = node
                .rules
                .as_ref()
                .map(profile::changed_parts)
                .unwrap_or_default();
            if !changed.is_empty() && self.graph.plan_resume(node.id)? {
                eprintln!(
                    "loom: 노드 {}의 시작 글이 바뀌었다({}). 턴 끝에 다시 띄운다",
                    node.id.0,
                    changed.join(", ")
                );
            }
            self.run_unless_busy(node.id);
        }
        Ok(())
    }

    pub(super) fn finish_stops(self: &Arc<Self>) -> rusqlite::Result<()> {
        for node in self.graph.active()? {
            let Some((phase, plan)) = self.graph.relaunch_plan(node.id)? else {
                continue;
            };
            if !matches!(phase, Phase::Stopping | Phase::Stopped) {
                continue;
            }
            eprintln!(
                "loom: 노드 {}의 옛 프로세스를 끝내던 중 데몬이 다시 시작했다. 다시 띄우기를 마친다",
                node.id.0
            );
            let app = self.clone();
            tokio::spawn(async move {
                let id = node.id;
                if let Err(e) = app.relaunch(node, plan, None).await {
                    eprintln!("loom: 노드 {}를 다시 띄우지 못했다: {e}", id.0);
                }
            });
        }
        Ok(())
    }

    pub(super) fn run_unless_busy(self: &Arc<Self>, node: NodeId) -> bool {
        let claimed = plan_written(node, self.graph.claim_idle_turn_end(node));
        let ran = self.run_claimed(node, None, claimed);
        if !ran {
            self.graph.wake_mail();
        }
        ran
    }

    pub(super) fn run_at_turn_end(self: &Arc<Self>, node: NodeId, guard: Option<Guard>) {
        let claimed = plan_written(node, self.graph.claim_turn_end(node));
        self.run_claimed(node, guard, claimed);
    }

    fn run_claimed(
        self: &Arc<Self>,
        node: NodeId,
        guard: Option<Guard>,
        claimed: Option<Plan>,
    ) -> bool {
        let Some(plan) = claimed else {
            return false;
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
        true
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
        plan_written(node, self.graph.defer_relaunch(node));
        self.graph.wake_mail();
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
        self.finish_plan(id);
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
        let (resume, site) = self
            .resumable(&node, profile, &settings)
            .await
            .map_err(|e| e.to_string())?;
        let brief = self.resumed_brief(&node, profile);
        let prepared = self
            .prepare_resumed(&node, &agent, profile, &resume, brief)
            .await?;
        if self.end_old(&node, prepared.discarder(), guard).await? == Stop::Kept {
            return Ok(Stop::Kept);
        }
        let old = self.record_of(&node);
        let _ = profile::adapter(profile).end_detached(&old, &site).await;
        if let Err(e) = Served::end_stored(&old).await {
            prepared.discarder()();
            let _ = self.graph.lost(node.id);
            return Err(format!("옛 세션이 끝나지 않았다: {e}"));
        }
        self.run_resumed(node.id, prepared)
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
        if let Err(e) = self.end_detached(&node).await {
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

    pub(super) async fn end_detached(&self, node: &Active) -> Result<(), String> {
        let Some(profile) = profile_of(node) else {
            return Ok(());
        };
        let Ok(place) = self.graph.place_of(node.id) else {
            return Ok(());
        };
        let Ok(site) = self.site_at(Path::new(&place.worktree)).await else {
            return Ok(());
        };
        profile::adapter(profile)
            .end_detached(&self.record_of(node), &site)
            .await
    }

    pub(super) async fn end_old(
        &self,
        node: &Active,
        discard: impl Fn(),
        guard: Option<Guard>,
    ) -> Result<Stop, String> {
        let mut exited = pin!(self.relaunch_exits.notified());
        exited.as_mut().enable();
        let snapshot = Snapshot::now();
        let shells = self.background_shells(&snapshot, node);
        let guarded = guard.is_some();
        let claimed = match guard {
            None => self.graph.claim_stop(node.id).map_err(|e| e.to_string())?,
            Some(keeps) => {
                let mut claimed = Ok(None);
                self.host.stop_if(node.id, |keys| {
                    if !keeps(&keys.screen()) {
                        return false;
                    }
                    claimed = self.graph.claim_stop(node.id);
                    matches!(claimed, Ok(Some(Phase::Running | Phase::Stopping)))
                });
                match claimed.map_err(|e| e.to_string())? {
                    None => {
                        discard();
                        return Ok(Stop::Kept);
                    }
                    claimed => claimed,
                }
            }
        };
        match claimed {
            Some(Phase::Stopped) => return Ok(Stop::Stopped),
            Some(Phase::Running | Phase::Stopping) => {}
            Some(Phase::AtTurnEnd) | None => {
                discard();
                return Err(format!("노드 {}의 다시 띄우기 계획이 없다", node.id.0));
            }
        }
        self.feed(node.id, AgentEvent::RelaunchStarted);
        snapshot.kill_trees(shells);
        self.idle.end(node.id, IdleEnd::Relaunch);
        if !guarded {
            self.host.stop(node.id);
        }
        let deadline = tokio::time::Instant::now() + EXIT_WAIT;
        loop {
            if self.pending(node.id) == Some(Phase::Stopped) {
                return Ok(Stop::Stopped);
            }
            if tokio::time::timeout_at(deadline, exited.as_mut())
                .await
                .is_err()
            {
                break;
            }
            exited.set(self.relaunch_exits.notified());
            exited.as_mut().enable();
        }
        if !self
            .graph
            .stop_dropped(node.id)
            .map_err(|e| e.to_string())?
        {
            return Ok(Stop::Stopped);
        }
        discard();
        Err(format!("노드 {}의 옛 프로세스가 끝나지 않았다", node.id.0))
    }

    async fn prepare_other(
        self: &Arc<Self>,
        node: &Active,
        to: &NewAgent,
    ) -> Result<super::Prepared, String> {
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
        let adapter = profile::adapter(profile_of(node)?);
        adapter.record(&self.record_of(node)).await
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

    pub(super) async fn revive(
        self: &Arc<Self>,
        node: Active,
        notice: &str,
    ) -> Result<(), ResumeError> {
        let (prepared, profile, site) = self.prepare_revive(&node, notice).await?;
        let _ = profile::adapter(profile)
            .end_detached(&self.record_of(&node), &site)
            .await;
        Ok(self.run_resumed(node.id, prepared).await?)
    }

    pub(super) async fn revive_detached_ended(
        self: &Arc<Self>,
        node: Active,
        notice: &str,
    ) -> Result<(), ResumeError> {
        let (prepared, ..) = self.prepare_revive(&node, notice).await?;
        Ok(self.run_resumed(node.id, prepared).await?)
    }

    async fn prepare_revive(
        self: &Arc<Self>,
        node: &Active,
        notice: &str,
    ) -> Result<(super::Prepared, Profile, Site), ResumeError> {
        let (Some(agent), Some(profile)) = (node.agent.clone(), profile_of(node)) else {
            return Err(ResumeError::NoSession(
                "프로필이 있는 에이전트가 아니다".into(),
            ));
        };
        let settings = self.settings.get().map_err(|e| e.to_string())?;
        let (resume, site) = self.resumable(node, profile, &settings).await?;
        let prepared = self
            .prepare_resumed(node, &agent, profile, &resume, notice.to_owned())
            .await?;
        Ok((prepared, profile, site))
    }

    async fn resumable(
        &self,
        node: &Active,
        profile: Profile,
        settings: &SettingsView,
    ) -> Result<(Session, Site), ResumeError> {
        let session = node.session.clone().ok_or_else(|| {
            ResumeError::NoSession(format!("노드 {}에는 이어 갈 세션이 없다", node.id.0))
        })?;
        let place = self.graph.place_of(node.id).map_err(|e| e.to_string())?;
        let cwd = Path::new(&place.worktree);
        let site = self.site_at(cwd).await.map_err(|e| format!("{e:?}"))?;
        if let Some(provider) = profile.provider() {
            let carry = Carry {
                site: &site,
                from: node.account.as_ref(),
                to: settings.account(provider, site.distro()),
                cwd,
            };
            profile::adapter(profile).carry(&session, &carry)?;
        }
        Ok((session, site))
    }

    async fn prepare_resumed(
        self: &Arc<Self>,
        node: &Active,
        agent: &AgentId,
        profile: Profile,
        resume: &Session,
        brief: String,
    ) -> Result<super::Prepared, String> {
        let choice = ModelChoice {
            model: node.model.model.clone(),
            effort: node.model.effort.clone(),
        };
        let start = Start::Agent {
            agent: agent.clone(),
            brief,
            choice: match profile::model_rule(Some(profile)) {
                ModelRule::Free => choice,
                ModelRule::EffortNeedsModel if choice.model.is_some() => choice,
                ModelRule::Fixed | ModelRule::EffortNeedsModel => ModelChoice::default(),
            },
            resume: Some(resume.clone()),
        };
        self.prepare_start(node.id, &start).await
    }

    async fn prepare_start(
        self: &Arc<Self>,
        id: NodeId,
        start: &Start,
    ) -> Result<super::Prepared, String> {
        let place = self.graph.place_of(id).map_err(|e| e.to_string())?;
        let position = self.position_of(id).map_err(|e| e.to_string())?;
        let task = self.task_dir(id).await.map_err(|e| format!("{e:?}"))?;
        self.prepare(start, Path::new(&place.worktree), position, Some(&task))
            .await
            .map_err(|e| format!("{e:?}"))
    }

    async fn run_resumed(
        self: &Arc<Self>,
        id: NodeId,
        mut prepared: super::Prepared,
    ) -> Result<(), String> {
        let discard = prepared.discarder();
        if let Err(e) = prepared.before_start().await {
            discard();
            let _ = self.graph.lost(id);
            return Err(e);
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
        let opens_no_wait = !profile.listens_at_start();
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
            .filter(|lines| !lines.iter().take(2).any(|argv| profile::runs_helper(argv)))
            .filter_map(|lines| {
                lines
                    .get(1)
                    .or(lines.first())
                    .map(|argv| shown(&argv.join(" ")))
            })
            .collect()
    }

    pub(super) fn background_shells(&self, snapshot: &Snapshot, node: &Active) -> Vec<u32> {
        let helper = profile_of(node)
            .and_then(|profile| profile::adapter(profile).shell_root(&self.record_of(node)));
        [self.host.pid(node.id), helper]
            .into_iter()
            .flatten()
            .flat_map(|root| snapshot.session_leaders_under(root))
            .collect()
    }
}

fn plan_written<R: Default>(node: NodeId, written: rusqlite::Result<R>) -> R {
    written
        .inspect_err(|e| {
            eprintln!(
                "loom: 노드 {}의 다시 띄우기 계획을 쓰지 못했다: {e}",
                node.0
            )
        })
        .unwrap_or_default()
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

fn shown(line: &str) -> String {
    match line.char_indices().nth(STOPPED_LINE_CHARS) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line.to_owned(),
    }
}
