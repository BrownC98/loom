use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use crate::graph::NodeId;

#[derive(Default)]
pub struct IdleWaits {
    ends: Mutex<HashMap<NodeId, Arc<Notify>>>,
    turns: Mutex<HashSet<NodeId>>,
}

impl IdleWaits {
    pub fn open(&self, node: NodeId) -> Arc<Notify> {
        self.turns.lock().unwrap().remove(&node);
        let end = Arc::new(Notify::new());
        if let Some(old) = self.ends.lock().unwrap().insert(node, end.clone()) {
            old.notify_one();
        }
        end
    }

    pub fn reopen(&self, node: NodeId) -> Option<Arc<Notify>> {
        if self.in_turn(node) {
            return None;
        }
        let mut ends = self.ends.lock().unwrap();
        if ends.contains_key(&node) {
            return None;
        }
        let end = Arc::new(Notify::new());
        ends.insert(node, end.clone());
        Some(end)
    }

    pub fn in_turn(&self, node: NodeId) -> bool {
        self.turns.lock().unwrap().contains(&node)
    }

    pub fn waiting(&self, node: NodeId) -> bool {
        self.ends.lock().unwrap().contains_key(&node)
    }

    pub fn turn_started(&self, node: NodeId) {
        self.end(node);
        self.turns.lock().unwrap().insert(node);
    }

    pub fn end(&self, node: NodeId) {
        self.turns.lock().unwrap().remove(&node);
        if let Some(old) = self.ends.lock().unwrap().remove(&node) {
            old.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_turn_hook_until_the_turn_end_counts_as_a_turn() {
        let waits = IdleWaits::default();
        assert!(!waits.in_turn(NodeId(1)));
        waits.turn_started(NodeId(1));
        assert!(waits.in_turn(NodeId(1)));
        let _ = waits.open(NodeId(1));
        assert!(!waits.in_turn(NodeId(1)));
        waits.turn_started(NodeId(1));
        waits.end(NodeId(1));
        assert!(!waits.in_turn(NodeId(1)));
    }

    #[test]
    fn only_an_open_wait_is_waiting() {
        let waits = IdleWaits::default();
        assert!(!waits.waiting(NodeId(1)));
        let _ = waits.open(NodeId(1));
        assert!(waits.waiting(NodeId(1)));
        waits.turn_started(NodeId(1));
        assert!(!waits.waiting(NodeId(1)));
    }

    #[test]
    fn an_ended_wait_reopens_only_when_no_newer_wait_or_turn_came() {
        let waits = IdleWaits::default();
        let _ = waits.open(NodeId(1));
        waits.end(NodeId(1));
        assert!(waits.reopen(NodeId(1)).is_some());
        assert!(waits.reopen(NodeId(1)).is_none());
        waits.turn_started(NodeId(1));
        assert!(waits.reopen(NodeId(1)).is_none());
    }

    #[tokio::test]
    async fn a_newer_wait_or_a_turn_ends_the_old_wait() {
        let waits = IdleWaits::default();
        let first = waits.open(NodeId(1));
        let second = waits.open(NodeId(1));
        first.notified().await;
        let other = waits.open(NodeId(2));
        waits.end(NodeId(1));
        second.notified().await;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), other.notified())
                .await
                .is_err()
        );
    }
}
