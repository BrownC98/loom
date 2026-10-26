use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;

use crate::graph::{AgentEvent, AgentState, Entry, Graph, IdleEnd, NodeId, Who};

pub struct IdleWaits {
    graph: Arc<Graph>,
    ends: Mutex<HashMap<NodeId, Latest>>,
    next_wait: AtomicU64,
}

struct Latest {
    wait: u64,
    end: Option<oneshot::Sender<IdleEnd>>,
}

pub struct Wait<'a> {
    waits: &'a IdleWaits,
    node: NodeId,
    id: u64,
    who: Option<Who>,
    end: oneshot::Receiver<IdleEnd>,
}

impl IdleWaits {
    pub fn new(graph: Arc<Graph>) -> Self {
        Self {
            graph,
            ends: Mutex::default(),
            next_wait: AtomicU64::new(0),
        }
    }

    pub fn open(&self, node: NodeId) -> Wait<'_> {
        self.open_by(node, AgentEvent::WaitOpened)
    }

    pub fn open_after_turn(&self, node: NodeId) -> Wait<'_> {
        self.open_by(node, AgentEvent::TurnEndedWaitOpened)
    }

    fn open_by(&self, node: NodeId, event: AgentEvent) -> Wait<'_> {
        let id = self.next_wait.fetch_add(1, Ordering::Relaxed);
        let (tx, end) = oneshot::channel();
        let mut ends = self.ends.lock().unwrap();
        let latest = Latest {
            wait: id,
            end: Some(tx),
        };
        if let Some(old) = ends.insert(node, latest).and_then(|old| old.end) {
            let _ = old.send(IdleEnd::NewWait);
        }
        self.feed(node, event);
        drop(ends);
        let who = self
            .graph
            .who(node)
            .inspect_err(|e| eprintln!("loom: 노드 {}의 쉬는 대기를 남기지 못했다: {e}", node.0))
            .ok();
        if let Some(who) = &who {
            self.record(Entry::IdleOpened { node: who.clone() });
        }
        Wait {
            waits: self,
            node,
            id,
            who,
            end,
        }
    }

    pub fn reopen(&self, wait: &mut Wait<'_>) -> bool {
        let mut ends = self.ends.lock().unwrap();
        let replaced = ends
            .get(&wait.node)
            .is_some_and(|latest| latest.wait != wait.id || latest.end.is_some());
        if replaced || self.in_turn(wait.node) {
            return false;
        }
        let (tx, end) = oneshot::channel();
        let latest = Latest {
            wait: wait.id,
            end: Some(tx),
        };
        ends.insert(wait.node, latest);
        self.feed(wait.node, AgentEvent::WaitOpened);
        wait.end = end;
        true
    }

    pub fn lose_unheld(&self, node: NodeId) {
        let ends = self.ends.lock().unwrap();
        let listening = self
            .graph
            .agent_state(node)
            .is_ok_and(|state| state == Some(AgentState::Listening));
        if listening && !held(&ends, node) {
            self.feed(node, AgentEvent::WaitClosed { by: IdleEnd::Gone });
        }
    }

    fn in_turn(&self, node: NodeId) -> bool {
        self.graph
            .agent_state(node)
            .is_ok_and(|state| state == Some(AgentState::Turn))
    }

    pub fn turn_started(&self, node: NodeId, hook: String) {
        let mut ends = self.ends.lock().unwrap();
        if let Some(old) = take_end(&mut ends, node) {
            let _ = old.send(IdleEnd::Turn { hook });
        }
        self.feed(node, AgentEvent::TurnStarted);
        drop(ends);
    }

    pub fn end(&self, node: NodeId, by: IdleEnd) {
        self.close_latest(node, by);
    }

    pub fn release(&self, node: NodeId) {
        self.close_latest(node, IdleEnd::Relaunch);
    }

    fn close_latest(&self, node: NodeId, by: IdleEnd) {
        let mut ends = self.ends.lock().unwrap();
        let Some(end) = take_end(&mut ends, node) else {
            return;
        };
        if !end.is_closed() {
            self.feed(node, AgentEvent::WaitClosed { by: by.clone() });
        }
        let _ = end.send(by);
    }

    fn record(&self, entry: Entry) {
        if let Err(e) = self.graph.record(&entry) {
            eprintln!("loom: 쉬는 대기를 남기지 못했다: {e}");
        }
    }

    fn feed(&self, node: NodeId, event: AgentEvent) {
        if let Err(e) = self.graph.agent_event(node, event) {
            eprintln!("loom: 노드 {}의 에이전트 상태를 쓰지 못했다: {e}", node.0);
        }
    }
}

fn held(ends: &HashMap<NodeId, Latest>, node: NodeId) -> bool {
    ends.get(&node)
        .and_then(|latest| latest.end.as_ref())
        .is_some_and(|end| !end.is_closed())
}

fn take_end(ends: &mut HashMap<NodeId, Latest>, node: NodeId) -> Option<oneshot::Sender<IdleEnd>> {
    ends.get_mut(&node).and_then(|latest| latest.end.take())
}

impl Wait<'_> {
    pub async fn ended(&mut self) -> IdleEnd {
        (&mut self.end)
            .await
            .expect("every removed end sends its cause")
    }

    pub fn finish(mut self, by: IdleEnd) {
        self.record(by);
    }

    pub fn hand_mail(mut self) {
        self.feed_if_latest(IdleEnd::Mail);
        self.record(IdleEnd::Mail);
    }

    fn record(&mut self, by: IdleEnd) {
        if let Some(node) = self.who.take() {
            if matches!(by, IdleEnd::Gone | IdleEnd::Error) {
                self.feed_if_latest(by.clone());
            }
            self.waits.record(Entry::IdleEnded { node, by });
        }
    }

    fn feed_if_latest(&self, by: IdleEnd) {
        let ends = self.waits.ends.lock().unwrap();
        if ends.get(&self.node).map(|latest| latest.wait) == Some(self.id) {
            self.waits.feed(self.node, AgentEvent::WaitClosed { by });
        }
    }
}

impl Drop for Wait<'_> {
    fn drop(&mut self) {
        self.record(IdleEnd::Gone);
    }
}
