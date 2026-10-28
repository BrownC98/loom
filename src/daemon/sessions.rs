use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use tokio::sync::watch;

use crate::graph::{AgentState, NodeId};
use crate::protocol::Handoff;
use crate::screen::SessionStart;

#[derive(Default)]
pub struct Sessions {
    nodes: Mutex<HashMap<NodeId, (String, watch::Sender<SessionStart>)>>,
}

impl Sessions {
    pub fn launched(&self, node: NodeId, token: &str) -> watch::Receiver<SessionStart> {
        let (session, started) = watch::channel(SessionStart::NotStarted);
        self.nodes
            .lock()
            .unwrap()
            .insert(node, (token.to_owned(), session));
        started
    }

    pub fn started(&self, node: NodeId, token: &str) {
        if let Some((_, session)) = self
            .nodes
            .lock()
            .unwrap()
            .get(&node)
            .filter(|(current, _)| current == token)
        {
            session.send_if_modified(|s| {
                std::mem::replace(s, SessionStart::Started) != SessionStart::Started
            });
        }
    }

    pub fn exited(&self, node: NodeId) {
        self.nodes.lock().unwrap().remove(&node);
    }
}

#[derive(Default)]
pub struct FirstInputs(Mutex<HashSet<NodeId>>);

impl FirstInputs {
    pub fn mark(&self, node: NodeId) {
        self.0.lock().unwrap().insert(node);
    }

    pub fn clear(&self, node: NodeId) {
        self.0.lock().unwrap().remove(&node);
    }

    pub fn injected(&self, node: NodeId) -> bool {
        self.0.lock().unwrap().remove(&node)
    }

    pub fn pending(&self, node: NodeId) -> bool {
        self.0.lock().unwrap().contains(&node)
    }
}

#[derive(Default)]
pub struct Wakes {
    nodes: Mutex<HashMap<NodeId, NodeWakes>>,
    order: Mutex<()>,
    handoffs: AtomicU64,
}

pub struct Order<'a> {
    _held: MutexGuard<'a, ()>,
}

#[derive(Default)]
struct NodeWakes {
    prompts: HashSet<String>,
    finish: Option<Finish>,
}

enum Finish {
    Touched {
        state: Option<AgentState>,
        handoff: Option<Handoff>,
    },
    Answered(Option<AgentState>),
}

impl Wakes {
    pub fn order(&self) -> Order<'_> {
        Order {
            _held: self.order.lock().unwrap(),
        }
    }

    pub fn woken(&self, _: &Order<'_>, node: NodeId, prompt: Option<String>) {
        if let Some(prompt) = prompt {
            self.nodes
                .lock()
                .unwrap()
                .entry(node)
                .or_default()
                .prompts
                .insert(prompt);
        }
    }

    pub fn submitted(&self, _: &Order<'_>, node: NodeId, prompt: &str) {
        if let Some(wakes) = self.nodes.lock().unwrap().get_mut(&node) {
            wakes.prompts.remove(prompt);
        }
    }

    pub fn ended_before(&self, _: &Order<'_>, node: NodeId, prompt: Option<&str>) -> bool {
        let mut nodes = self.nodes.lock().unwrap();
        match (nodes.get_mut(&node), prompt) {
            (Some(wakes), Some(prompt)) => wakes.prompts.remove(prompt),
            _ => false,
        }
    }

    pub fn forget(&self, node: NodeId) {
        self.nodes.lock().unwrap().remove(&node);
    }

    pub fn ask_finish(&self, node: NodeId, state: Option<AgentState>) -> bool {
        let mut nodes = self.nodes.lock().unwrap();
        let finish = &mut nodes.entry(node).or_default().finish;
        if let Some(Finish::Touched { state: asked, .. } | Finish::Answered(asked)) = finish
            && *asked == state
        {
            return false;
        }
        *finish = Some(Finish::Touched {
            state,
            handoff: None,
        });
        true
    }

    pub fn finish_handed(&self, node: NodeId, state: Option<AgentState>) -> Option<Handoff> {
        let mut nodes = self.nodes.lock().unwrap();
        let finish = &mut nodes.entry(node).or_default().finish;
        if matches!(finish, Some(Finish::Answered(shown)) if *shown == state) {
            return None;
        }
        let state = match finish.take() {
            Some(Finish::Touched { state, .. }) => state,
            _ => state,
        };
        let handoff = Handoff(self.handoffs.fetch_add(1, Ordering::Relaxed));
        *finish = Some(Finish::Touched {
            state,
            handoff: Some(handoff),
        });
        Some(handoff)
    }

    pub fn finish_shown(&self, node: NodeId, shown: Handoff) {
        let mut nodes = self.nodes.lock().unwrap();
        let Some(wakes) = nodes.get_mut(&node) else {
            return;
        };
        if let Some(Finish::Touched {
            state,
            handoff: Some(handoff),
        }) = &wakes.finish
            && *handoff == shown
        {
            wakes.finish = Some(Finish::Answered(state.clone()));
        }
    }

    pub fn finish_done(&self, node: NodeId) {
        if let Some(wakes) = self.nodes.lock().unwrap().get_mut(&node) {
            wakes.finish = None;
        }
    }

    pub fn heard(&self, node: NodeId) -> bool {
        let mut nodes = self.nodes.lock().unwrap();
        let Some(wakes) = nodes.get_mut(&node) else {
            return false;
        };
        let unanswered = matches!(wakes.finish, Some(Finish::Touched { .. }));
        if unanswered {
            wakes.finish = None;
        }
        unanswered
    }
}
