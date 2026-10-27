use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use tokio::sync::watch;

use crate::graph::NodeId;
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
