use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;

use crate::graph::{AgentEvent, Entry, Graph, IdleEnd, IdleId, NodeId, Who};

pub struct IdleWaits {
    graph: Arc<Graph>,
    ends: Mutex<HashMap<IdleId, oneshot::Sender<IdleEnd>>>,
}

pub struct Wait<'a> {
    waits: &'a IdleWaits,
    node: NodeId,
    id: IdleId,
    who: Option<Who>,
    open: bool,
    end: oneshot::Receiver<IdleEnd>,
}

impl IdleWaits {
    pub fn new(graph: Arc<Graph>) -> Self {
        Self {
            graph,
            ends: Mutex::default(),
        }
    }

    pub fn open(&self, node: NodeId) -> rusqlite::Result<Wait<'_>> {
        self.open_by(node, AgentEvent::WaitOpened)
    }

    pub fn open_after_turn(&self, node: NodeId) -> rusqlite::Result<Wait<'_>> {
        self.open_by(node, AgentEvent::TurnEndedWaitOpened)
    }

    fn open_by(&self, node: NodeId, event: AgentEvent) -> rusqlite::Result<Wait<'_>> {
        let (tx, end) = oneshot::channel();
        let mut ends = self.ends.lock().unwrap();
        let (id, replaced) = self.graph.open_idle(node, event)?;
        if let Some(old) = replaced.and_then(|old| ends.remove(&old)) {
            let _ = old.send(IdleEnd::NewWait);
        }
        ends.insert(id, tx);
        drop(ends);
        let who = self
            .graph
            .who(node)
            .inspect_err(|e| eprintln!("loom: 노드 {}의 쉬는 대기를 남기지 못했다: {e}", node.0))
            .ok();
        if let Some(who) = &who {
            self.record(Entry::IdleOpened { node: who.clone() });
        }
        Ok(Wait {
            waits: self,
            node,
            id,
            who,
            open: true,
            end,
        })
    }

    pub fn reopen(&self, wait: &mut Wait<'_>) -> bool {
        let mut ends = self.ends.lock().unwrap();
        match self.graph.reopen_idle(wait.node, wait.id) {
            Ok(true) => {}
            Ok(false) => return false,
            Err(e) => {
                eprintln!(
                    "loom: 노드 {}의 쉬는 대기를 다시 열지 못했다: {e}",
                    wait.node.0
                );
                return false;
            }
        }
        let (tx, end) = oneshot::channel();
        ends.insert(wait.id, tx);
        wait.end = end;
        true
    }

    pub fn turn_started(&self, node: NodeId, step: String) {
        let mut ends = self.ends.lock().unwrap();
        let ended = self.graph.idle_turn(node);
        self.wake(&mut ends, node, ended, IdleEnd::Turn { step });
    }

    pub fn end(&self, node: NodeId, by: IdleEnd) {
        let mut ends = self.ends.lock().unwrap();
        let ended = self.graph.end_idle(node, by.clone());
        self.wake(&mut ends, node, ended, by);
    }

    pub fn submit_input(&self, node: NodeId) {
        let mut ends = self.ends.lock().unwrap();
        let ended = self.graph.submit_input(node);
        self.wake(&mut ends, node, ended, IdleEnd::Typed);
    }

    pub fn release_submitted(&self, node: NodeId) {
        let mut ends = self.ends.lock().unwrap();
        let ended = self.graph.release_submitted(node);
        self.wake(&mut ends, node, ended, IdleEnd::Typed);
    }

    pub fn release(&self, node: NodeId) {
        self.end(node, IdleEnd::Relaunch);
    }

    fn wake(
        &self,
        ends: &mut HashMap<IdleId, oneshot::Sender<IdleEnd>>,
        node: NodeId,
        ended: rusqlite::Result<Option<IdleId>>,
        by: IdleEnd,
    ) {
        match ended {
            Ok(id) => {
                if let Some(end) = id.and_then(|id| ends.remove(&id)) {
                    let _ = end.send(by);
                }
            }
            Err(e) => eprintln!("loom: 노드 {}의 에이전트 상태를 쓰지 못했다: {e}", node.0),
        }
    }

    fn record(&self, entry: Entry) {
        if let Err(e) = self.graph.record(&entry) {
            eprintln!("loom: 쉬는 대기를 남기지 못했다: {e}");
        }
    }
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
        self.close(IdleEnd::Mail);
        self.record(IdleEnd::Mail);
    }

    fn record(&mut self, by: IdleEnd) {
        if !std::mem::take(&mut self.open) {
            return;
        }
        if matches!(by, IdleEnd::Gone | IdleEnd::Error) {
            self.close(by.clone());
        }
        if let Some(node) = self.who.take() {
            self.waits.record(Entry::IdleEnded { node, by });
        }
    }

    fn close(&self, by: IdleEnd) {
        let mut ends = self.waits.ends.lock().unwrap();
        ends.remove(&self.id);
        if let Err(e) = self.waits.graph.close_idle(self.node, self.id, by) {
            eprintln!(
                "loom: 노드 {}의 에이전트 상태를 쓰지 못했다: {e}",
                self.node.0
            );
        }
    }
}

impl Drop for Wait<'_> {
    fn drop(&mut self) {
        self.record(IdleEnd::Gone);
    }
}
