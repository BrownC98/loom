use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use tokio::sync::watch;

use crate::graph::{AgentState, NodeId};
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
pub struct Wakes(Mutex<HashMap<NodeId, NodeWakes>>);

#[derive(Default)]
struct NodeWakes {
    prompts: HashSet<String>,
    finish: Option<Finish>,
}

enum Finish {
    Touched(Option<AgentState>),
    Answered(Option<AgentState>),
}

impl Wakes {
    pub fn woken(&self, node: NodeId, prompt: Option<String>) {
        if let Some(prompt) = prompt {
            self.0
                .lock()
                .unwrap()
                .entry(node)
                .or_default()
                .prompts
                .insert(prompt);
        }
    }

    pub fn ended_before(&self, node: NodeId, prompt: Option<&str>) -> bool {
        let mut wakes = self.0.lock().unwrap();
        match (wakes.get_mut(&node), prompt) {
            (Some(wakes), Some(prompt)) => wakes.prompts.remove(prompt),
            _ => false,
        }
    }

    pub fn forget(&self, node: NodeId) {
        self.0.lock().unwrap().remove(&node);
    }

    pub fn ask_finish(&self, node: NodeId, state: Option<AgentState>) -> bool {
        let mut wakes = self.0.lock().unwrap();
        let finish = &mut wakes.entry(node).or_default().finish;
        if let Some(Finish::Touched(asked) | Finish::Answered(asked)) = finish
            && *asked == state
        {
            return false;
        }
        *finish = Some(Finish::Touched(state));
        true
    }

    pub fn finish_answered(&self, node: NodeId, state: Option<AgentState>) {
        self.0.lock().unwrap().entry(node).or_default().finish = Some(Finish::Answered(state));
    }

    pub fn finish_done(&self, node: NodeId) {
        if let Some(wakes) = self.0.lock().unwrap().get_mut(&node) {
            wakes.finish = None;
        }
    }

    pub fn heard(&self, node: NodeId) -> bool {
        let mut wakes = self.0.lock().unwrap();
        let Some(wakes) = wakes.get_mut(&node) else {
            return false;
        };
        let unanswered = matches!(wakes.finish, Some(Finish::Touched(_)));
        if unanswered {
            wakes.finish = None;
        }
        unanswered
    }
}
