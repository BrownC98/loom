use std::pin::pin;
use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;
use tokio::sync::watch;

use super::idle::Wait;
use super::relaunch::profile_of;
use super::{AgentError, App, for_agent};
use crate::graph::{AgentEvent, IdleEnd, NodeId, WaitSource};
use crate::profile::adapter::Port;
use crate::profile::{self, BATCH_LIMIT_BYTES, Profile};
use crate::protocol::{
    Delivery, EndedTurn, IdleReply, IdleRequest, Kind, MessageId, Refusal, Report, Session,
    TakeRequest, Taken,
};
use axum::http::StatusCode;

impl App {
    pub(super) fn take_now(
        self: &Arc<Self>,
        node: NodeId,
        token: &str,
        req: TakeRequest,
    ) -> Result<Taken, Refusal> {
        self.idle.turn_started(node, req.step);
        self.turn_ends.stepped(node, req.turn.as_deref());
        self.keep_session(token, req.session.as_ref())?;
        let messages = self.unheld(node, || self.graph.take(node, &req.kinds, req.limit_bytes))?;
        let note = match req.human_input {
            true => self.human_input(node, req.prompt.as_deref())?,
            false => None,
        };
        Ok(Taken { messages, note })
    }

    pub(super) fn take_mail(
        &self,
        node: NodeId,
        kinds: &[Kind],
        limit_bytes: usize,
    ) -> Result<Vec<Delivery>, Refusal> {
        self.unheld(node, || self.graph.take_waking(node, kinds, limit_bytes))
    }

    fn unheld(
        &self,
        node: NodeId,
        take: impl FnOnce() -> Result<Vec<Delivery>, Refusal>,
    ) -> Result<Vec<Delivery>, Refusal> {
        if self.holds_mail(node) {
            return Ok(Vec::new());
        }
        take()
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

    pub(super) fn human_input(
        self: &Arc<Self>,
        node: NodeId,
        prompt: Option<&str>,
    ) -> Result<Option<String>, Refusal> {
        self.graph.human_input(node)?;
        self.draft_ended(node);
        self.turn_ends.human(node);
        if let Some(text) = prompt
            && !self.first_inputs.injected(node)
        {
            self.graph.human_typed(node, text)?;
        }
        Ok(profile::questions_note(&self.graph.open_questions(node)?))
    }

    pub(super) fn turn_ended(
        self: &Arc<Self>,
        node: NodeId,
        ended: EndedTurn,
    ) -> rusqlite::Result<Option<String>> {
        if let Some(turn) = ended.turn.as_deref()
            && self.graph.turn_was_ended(node, turn)?
        {
            return Ok(None);
        }
        if let Some(text) = ended.reply {
            self.turn_ends.said(node, &text, None);
        }
        let nudge = self.turn_end(node);
        if !matches!(nudge, Ok(Some(_))) {
            self.run_at_turn_end(node, None);
        }
        let nudge = nudge?;
        self.turn_ends.stopped(node, ended.turn.as_deref());
        Ok(nudge.map(str::to_owned))
    }

    pub(super) async fn wait_idle(
        self: &Arc<Self>,
        node: NodeId,
        token: &str,
        req: IdleRequest,
        arrival: Option<super::arrival::Arrival>,
        current: impl Fn() -> bool,
    ) -> Result<IdleReply, AgentError> {
        let opened = self.open_wait(node, token, req, arrival)?;
        self.listen(node, opened, current).await
    }

    fn open_wait(
        self: &Arc<Self>,
        node: NodeId,
        token: &str,
        req: IdleRequest,
        arrival: Option<super::arrival::Arrival>,
    ) -> Result<Opened<'_>, AgentError> {
        self.keep_session(token, req.session.as_ref())
            .map_err(for_agent)?;
        let after_turn = req.turn_end.is_some();
        if let Some(ended) = req.turn_end
            && let Some(nudge) = self
                .turn_ended(node, ended)
                .map_err(|e| for_agent(Refusal::from(e)))?
        {
            return Ok(Opened::Nudge(nudge));
        }
        let wait = match after_turn {
            true => self.idle.open_after_turn(node),
            false => self.idle.open(node),
        }
        .map_err(|e| for_agent(Refusal::from(e)))?;
        if let Some(arrival) = arrival {
            arrival.arrived();
        }
        let alive = match self.graph.relations(node) {
            Ok(relations) => relations.me.alive,
            Err(e) => {
                wait.finish(IdleEnd::Error);
                return Err(for_agent(e));
            }
        };
        if !alive {
            self.idle.end(node, IdleEnd::Exit);
            wait.finish(IdleEnd::Exit);
            eprintln!(
                "loom: 노드 {}의 쉬는 대기를 거절했다: 프로세스가 끝났다",
                node.0
            );
            return Err((StatusCode::GONE, "The node process has exited.".to_owned()));
        }
        if req.rate_limited
            && let Some(provider) = self.provider(node)
        {
            self.rate_limited(node, provider);
        }
        if !after_turn {
            self.run_at_turn_end(node, None);
        }
        Ok(Opened::Wait(wait))
    }

    async fn listen(
        &self,
        node: NodeId,
        opened: Opened<'_>,
        current: impl Fn() -> bool,
    ) -> Result<IdleReply, AgentError> {
        let mut wait = match opened {
            Opened::Nudge(nudge) => return Ok(IdleReply::Nudge { nudge }),
            Opened::Wait(wait) => wait,
        };
        while self.holds_mail(node) {
            let by = wait.ended().await;
            if !current() || !self.idle.reopen(&mut wait) {
                wait.finish(by);
                return Ok(IdleReply::Mail(Vec::new()));
            }
        }
        tokio::select! {
            taken = self.next_mail(node, Kind::ALL, BATCH_LIMIT_BYTES) => match taken {
                Ok(taken) => {
                    wait.hand_mail();
                    Ok(IdleReply::Mail(taken))
                }
                Err(e) => {
                    wait.finish(IdleEnd::Error);
                    eprintln!("loom: 노드 {}의 쉬는 대기가 끝났다: {e}", node.0);
                    Err(for_agent(e))
                }
            },
            by = wait.ended() => {
                wait.finish(by);
                Ok(IdleReply::Mail(Vec::new()))
            }
        }
    }

    pub(super) fn report(self: &Arc<Self>, node: NodeId, token: &str, report: Report) {
        let written = match report {
            Report::Model(model) => self.graph.set_model(node, &model),
            Report::Session(session) => {
                self.sessions.started(node, token);
                self.graph.set_session(token, &session)
            }
            Report::HumanWait(wait) => {
                let on_screen = wait.as_ref().is_some_and(|(_, _, prompt)| prompt.is_none());
                self.graph
                    .set_wait(
                        token,
                        WaitSource::Agent,
                        wait.as_ref()
                            .map(|(kind, text, prompt)| (*kind, text.as_str(), prompt.clone())),
                    )
                    .and_then(|()| match on_screen.then(|| self.profile(node)).flatten() {
                        Some(profile) => {
                            let prompt = profile::prompt(Some(profile), &self.host.screen(node));
                            self.graph.screen_prompt(token, prompt)
                        }
                        None => Ok(()),
                    })
            }
            Report::Said { text, turn } => {
                self.turn_ends.said(node, &text, turn.as_deref());
                Ok(())
            }
            Report::QuestionBox => {
                self.turn_ends.question_box(node);
                Ok(())
            }
            Report::RateLimited => {
                if let Some(provider) = self.provider(node) {
                    self.rate_limited(node, provider);
                }
                Ok(())
            }
            Report::TurnStopped { turn } => {
                self.feed(node, AgentEvent::TurnEnded);
                self.run_at_turn_end(node, None);
                self.turn_ends.stopped(node, turn.as_deref());
                self.turn_stopped(node)
            }
            Report::Busy => {
                self.feed(node, AgentEvent::Busy);
                Ok(())
            }
            Report::Lost => {
                self.feed(node, AgentEvent::Lost);
                self.graph.set_wait(token, WaitSource::Agent, None)
            }
            Report::Refused { ids, cause } => {
                match self.graph.release_refused(node, &ids) {
                    Ok(true) => self.end_refused(node, &cause),
                    Ok(false) => {}
                    Err(e) => eprintln!("loom: 노드 {}의 메시지를 큐로 돌리지 못했다: {e}", node.0),
                }
                Ok(())
            }
        };
        if let Err(e) = written {
            eprintln!("loom: 노드 {}의 알림을 쓰지 못했다: {e}", node.0);
        }
    }

    fn end_refused(&self, node: NodeId, cause: &str) {
        let notice =
            format!("The agent refused the same mail again after loom started it again: {cause}");
        if let Err(e) = self.graph.lost_because(node, notice) {
            eprintln!("loom: 노드 {}를 exited로 바꾸지 못했다: {e}", node.0);
        }
        self.host.stop(node);
    }

    fn keep_session(&self, token: &str, session: Option<&Session>) -> Result<(), Refusal> {
        match session {
            Some(session) => self
                .graph
                .set_session(token, session)
                .map_err(Refusal::from),
            None => Ok(()),
        }
    }

    fn provider(&self, node: NodeId) -> Option<crate::protocol::Provider> {
        self.profile(node).and_then(Profile::provider)
    }

    fn profile(&self, node: NodeId) -> Option<Profile> {
        let nodes = self
            .graph
            .active()
            .inspect_err(|e| eprintln!("loom: 노드 {}의 프로필을 읽지 못했다: {e}", node.0))
            .ok()?;
        nodes.iter().find(|n| n.id == node).and_then(profile_of)
    }
}

enum Opened<'a> {
    Nudge(String),
    Wait(Wait<'a>),
}

pub(super) struct AgentPort {
    app: Arc<App>,
    token: String,
    node: watch::Sender<Option<NodeId>>,
    early: Mutex<Vec<Report>>,
    ended: watch::Sender<bool>,
}

impl AgentPort {
    pub(super) fn new(app: Arc<App>, token: String) -> Arc<Self> {
        Arc::new(Self {
            app,
            token,
            node: watch::channel(None).0,
            early: Mutex::default(),
            ended: watch::channel(false).0,
        })
    }

    pub(super) fn bind(&self, node: NodeId) {
        let early = {
            let mut early = self.early.lock().unwrap();
            self.node.send_replace(Some(node));
            std::mem::take(&mut *early)
        };
        for report in early {
            self.apply(node, report);
        }
    }

    pub(super) fn end(&self) {
        self.ended.send_replace(true);
    }

    fn current(&self, node: NodeId) -> bool {
        !*self.ended.borrow() && self.app.graph.caller(&self.token).ok().flatten() == Some(node)
    }

    fn apply(&self, node: NodeId, report: Report) {
        if self.current(node) {
            self.app.report(node, &self.token, report);
        }
    }

    async fn bound(&self) -> NodeId {
        let mut node = self.node.subscribe();
        let bound = async { node.wait_for(Option::is_some).await.ok().and_then(|n| *n) };
        tokio::select! {
            Some(node) = bound => node,
            () = self.until_ended() => std::future::pending().await,
        }
    }

    async fn until_ended(&self) {
        let _ = self.ended.subscribe().wait_for(|e| *e).await;
    }
}

impl Port for AgentPort {
    fn take(&self, req: TakeRequest) -> Result<Taken, Refusal> {
        match *self.node.borrow() {
            Some(node) => self.app.take_now(node, &self.token, req),
            None => Ok(Taken::default()),
        }
    }

    fn idle(&self, req: IdleRequest) -> BoxFuture<'_, Result<IdleReply, String>> {
        let bound = *self.node.borrow();
        let Some(node) = bound else {
            return Box::pin(async move {
                let node = self.bound().await;
                let waited = self
                    .app
                    .wait_idle(node, &self.token, req, None, || self.current(node))
                    .await;
                waited.map_err(|(_, e)| e)
            });
        };
        let opened = self.app.open_wait(node, &self.token, req, None);
        Box::pin(async move {
            let waited = match opened {
                Ok(opened) => self.app.listen(node, opened, || self.current(node)).await,
                Err(e) => Err(e),
            };
            waited.map_err(|(_, e)| e)
        })
    }

    fn next_mail(&self, kinds: &'static [Kind]) -> BoxFuture<'_, Result<Vec<Delivery>, Refusal>> {
        Box::pin(async move {
            let node = self.bound().await;
            tokio::select! {
                taken = self.app.next_mail(node, kinds, BATCH_LIMIT_BYTES) => taken,
                () = self.until_ended() => std::future::pending().await,
            }
        })
    }

    fn ack(&self, ids: &[MessageId]) -> Result<(), Refusal> {
        match *self.node.borrow() {
            Some(node) => self.app.graph.ack(node, ids),
            None => Ok(()),
        }
    }

    fn release(&self, ids: &[MessageId]) -> Result<(), Refusal> {
        match *self.node.borrow() {
            Some(node) => self.app.graph.release(node, ids),
            None => Ok(()),
        }
    }

    fn report(&self, report: Report) {
        let node = {
            let mut early = self.early.lock().unwrap();
            match *self.node.borrow() {
                Some(node) => node,
                None => return early.push(report),
            }
        };
        self.apply(node, report);
    }

    fn ended(&self) -> BoxFuture<'_, ()> {
        Box::pin(self.until_ended())
    }
}
