use std::pin::pin;
use std::sync::Arc;

use super::relaunch::provider_of;
use super::{AgentError, App, EndedTurn, IdleReply, IdleRequest, TakeRequest, Taken, for_agent};
use crate::graph::{AgentEvent, AgentState, IdleEnd, NodeId};
use crate::profile::{self, BATCH_LIMIT_BYTES, Change, NotTaken, ServerAdapter, Signal, Status};
use crate::protocol::{Delivery, Kind, MessageId, Refusal, Session};
use axum::http::StatusCode;

impl App {
    pub(super) fn take_now(
        self: &Arc<Self>,
        node: NodeId,
        token: &str,
        req: TakeRequest,
    ) -> Result<Taken, Refusal> {
        self.idle.turn_started(node, req.hook);
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
        self.keep_session(token, req.session.as_ref())
            .map_err(for_agent)?;
        let after_turn = req.turn_end.is_some();
        if let Some(ended) = req.turn_end
            && let Some(nudge) = self
                .turn_ended(node, ended)
                .map_err(|e| for_agent(Refusal::from(e)))?
        {
            return Ok(IdleReply::Nudge { nudge });
        }
        let mut wait = match after_turn {
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

    pub(super) async fn deliver_loop(
        self: Arc<Self>,
        node: NodeId,
        mut server: impl ServerAdapter,
        current: impl Fn(&App) -> bool,
    ) {
        let mut seen = server.status();
        loop {
            if !current(&self) {
                return;
            }
            let mut mail = pin!(self.graph.mail_changed());
            mail.as_mut().enable();
            let change = match seen.take() {
                Some(now) => self.server_status(node, now),
                None => None,
            };
            match server.signal(change).await {
                Some(Signal::TurnEnded(reply)) => {
                    match self.turn_ended(node, EndedTurn { reply, turn: None }) {
                        Ok(Some(nudge)) => match server.start_turn(&nudge).await {
                            Ok(()) => self.feed(node, AgentEvent::InputSent),
                            Err(e) => {
                                eprintln!("loom: 노드 {}를 다시 일하게 하지 못했다: {e}", node.0)
                            }
                        },
                        Ok(None) => {}
                        Err(e) => eprintln!("loom: 노드 {}의 턴 표시를 읽지 못했다: {e}", node.0),
                    }
                }
                Some(Signal::Stopped) => self.run_at_turn_end(node, None),
                Some(Signal::HumanInput) => match self.human_input(node, None) {
                    Ok(Some(text)) => {
                        if let Err(e) = server.steer(&text).await {
                            eprintln!("loom: 노드 {}에 열린 질문을 알리지 못했다: {e}", node.0);
                        }
                    }
                    Ok(None) => {}
                    Err(e) => eprintln!("loom: 노드 {}의 사람 입력을 처리하지 못했다: {e}", node.0),
                },
                None => {}
            }
            if self.deliver_batch(node, &mut server).await {
                continue;
            }
            tokio::select! {
                () = mail => {}
                alive = server.changed() => match alive {
                    true => seen = server.status(),
                    false => {
                        if current(&self) {
                            self.feed(node, AgentEvent::ServerLost);
                        }
                        return;
                    }
                },
            }
        }
    }

    fn server_status(&self, node: NodeId, now: Status) -> Option<Change> {
        let event = match now {
            Status::Busy => AgentEvent::ServerBusy,
            Status::Idle => AgentEvent::ServerIdle,
        };
        match self.graph.agent_event(node, event) {
            Ok(before) => Some(Change {
                before: status_of(before.as_ref()),
                now,
            }),
            Err(e) => {
                eprintln!("loom: 노드 {}의 에이전트 상태를 쓰지 못했다: {e}", node.0);
                None
            }
        }
    }

    async fn deliver_batch(&self, node: NodeId, server: &mut impl ServerAdapter) -> bool {
        let now = status_of(self.agent_state(node).as_ref());
        let mut taken = Vec::new();
        for kinds in server.takes(now) {
            match self.take_mail(node, kinds, BATCH_LIMIT_BYTES) {
                Ok(found) if !found.is_empty() => {
                    taken = found;
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    eprintln!("loom: 노드 {}의 메일함을 읽지 못했다: {e}", node.0);
                    return false;
                }
            }
        }
        if taken.is_empty() {
            return false;
        }
        if taken.iter().any(|d| d.kind == Kind::Interrupt) {
            server.stop_shells().await;
        }
        let ids: Vec<MessageId> = taken.iter().map(|d| d.id).collect();
        match server.inject(&taken, now).await {
            Ok(()) => {
                if let Err(e) = self.graph.ack(node, &ids) {
                    eprintln!("loom: 노드 {}의 받음 확인을 쓰지 못했다: {e}", node.0);
                }
                self.feed(node, AgentEvent::InputSent);
                true
            }
            Err(e) => {
                eprintln!("loom: 노드 {}에 메시지를 넘기지 못했다: {e}", node.0);
                let released = match &e {
                    NotTaken::EventFollows(_) => self.graph.release(node, &ids).map(|()| false),
                    NotTaken::ServerStopped(_) => self.graph.release_refused(node, &ids),
                };
                match released {
                    Ok(false) => {}
                    Ok(true) => self.end_refused(node, &e),
                    Err(e) => eprintln!("loom: 노드 {}의 메시지를 큐로 돌리지 못했다: {e}", node.0),
                }
                false
            }
        }
    }

    fn end_refused(&self, node: NodeId, cause: &NotTaken) {
        let notice = format!(
            "The agent server refused the same mail again after loom started it again: {cause}"
        );
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
        let nodes = self
            .graph
            .active()
            .inspect_err(|e| eprintln!("loom: 노드 {}의 공급자를 읽지 못했다: {e}", node.0))
            .ok()?;
        nodes
            .into_iter()
            .find(|n| n.id == node)
            .as_ref()
            .and_then(provider_of)
    }
}

fn status_of(state: Option<&AgentState>) -> Status {
    match state {
        Some(AgentState::Turn) => Status::Busy,
        Some(AgentState::Blocked { before }) => status_of(Some(before)),
        _ => Status::Idle,
    }
}
