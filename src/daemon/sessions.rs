use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use tokio::sync::watch;

use crate::graph::NodeId;
use crate::screen::Session;

#[derive(Default)]
pub struct Sessions {
    nodes: Mutex<HashMap<NodeId, (String, watch::Sender<Session>)>>,
}

impl Sessions {
    pub fn launched(&self, node: NodeId, token: &str) -> watch::Receiver<Session> {
        let (session, started) = watch::channel(Session::NotStarted);
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
            session
                .send_if_modified(|s| std::mem::replace(s, Session::Started) != Session::Started);
        }
    }

    pub fn exited(&self, node: NodeId) {
        self.nodes.lock().unwrap().remove(&node);
    }
}

#[derive(Default)]
pub struct FirstInputs(Mutex<HashSet<NodeId>>);

impl FirstInputs {
    pub fn launched(&self, node: NodeId, first_input: bool) {
        let mut nodes = self.0.lock().unwrap();
        match first_input {
            true => nodes.insert(node),
            false => nodes.remove(&node),
        };
    }

    pub fn injected(&self, node: NodeId) -> bool {
        self.0.lock().unwrap().remove(&node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_late_call_of_the_old_process_does_not_start_the_new_session() {
        let sessions = Sessions::default();
        let old = sessions.launched(NodeId(1), "old");
        let new = sessions.launched(NodeId(1), "new");
        assert!(old.has_changed().is_err(), "옛 세션은 끝난다");
        sessions.started(NodeId(1), "old");
        assert!(*new.borrow() == Session::NotStarted);
        sessions.started(NodeId(1), "new");
        assert!(*new.borrow() == Session::Started);
    }

    #[test]
    fn only_the_first_prompt_after_a_launch_with_a_brief_is_injected() {
        let inputs = FirstInputs::default();
        inputs.launched(NodeId(1), true);
        inputs.launched(NodeId(2), false);
        assert!(inputs.injected(NodeId(1)));
        assert!(!inputs.injected(NodeId(1)), "the human typed this one");
        assert!(!inputs.injected(NodeId(2)));
    }
}
