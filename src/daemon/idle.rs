use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;

use crate::graph::{Entry, Graph, IdleEnd, NodeId, Who};

pub struct IdleWaits {
    graph: Arc<Graph>,
    ends: Mutex<HashMap<NodeId, oneshot::Sender<IdleEnd>>>,
    turns: Mutex<HashSet<NodeId>>,
}

pub struct Wait<'a> {
    waits: &'a IdleWaits,
    node: NodeId,
    end: oneshot::Receiver<IdleEnd>,
    recorded: bool,
}

impl IdleWaits {
    pub fn new(graph: Arc<Graph>) -> Self {
        Self {
            graph,
            ends: Mutex::default(),
            turns: Mutex::default(),
        }
    }

    pub fn open(&self, node: NodeId) -> Wait<'_> {
        self.turns.lock().unwrap().remove(&node);
        let (tx, end) = oneshot::channel();
        if let Some(old) = self.ends.lock().unwrap().insert(node, tx) {
            let _ = old.send(IdleEnd::NewWait);
        }
        self.record(node, |node| Entry::IdleOpened { node });
        Wait {
            waits: self,
            node,
            end,
            recorded: false,
        }
    }

    pub fn reopen(&self, wait: &mut Wait<'_>) -> bool {
        if self.in_turn(wait.node) {
            return false;
        }
        let mut ends = self.ends.lock().unwrap();
        if ends.contains_key(&wait.node) {
            return false;
        }
        let (tx, end) = oneshot::channel();
        ends.insert(wait.node, tx);
        wait.end = end;
        true
    }

    pub fn waiting(&self, node: NodeId) -> bool {
        self.ends.lock().unwrap().contains_key(&node)
    }

    pub fn in_turn(&self, node: NodeId) -> bool {
        self.turns.lock().unwrap().contains(&node)
    }

    pub fn turn_started(&self, node: NodeId, hook: String) {
        self.end(node, IdleEnd::Turn { hook });
        self.turns.lock().unwrap().insert(node);
    }

    pub fn end(&self, node: NodeId, by: IdleEnd) {
        self.turns.lock().unwrap().remove(&node);
        if let Some(old) = self.ends.lock().unwrap().remove(&node) {
            let _ = old.send(by);
        }
    }

    fn record(&self, node: NodeId, entry: impl FnOnce(Who) -> Entry) {
        if let Err(e) = self.graph.record_about(node, entry) {
            eprintln!("loom: 노드 {}의 쉬는 대기를 남기지 못했다: {e}", node.0);
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

    fn record(&mut self, by: IdleEnd) {
        self.recorded = true;
        self.waits
            .record(self.node, |node| Entry::IdleEnded { node, by });
    }
}

impl Drop for Wait<'_> {
    fn drop(&mut self) {
        if !self.recorded {
            self.record(IdleEnd::Gone);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Launched, Place};
    use crate::worktree::RepoId;

    fn waits() -> (tempfile::TempDir, [NodeId; 2], IdleWaits) {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::open(&dir.path().join("loom.db")).unwrap();
        db.lock()
            .execute("INSERT INTO repos (id, path) VALUES (1, '/repo')", [])
            .unwrap();
        let graph = Arc::new(Graph::open(db).unwrap());
        let place = Place {
            repo: RepoId(1),
            worktree: "/repo".into(),
        };
        let root = |token: &str| {
            let launched = Launched {
                token: token.into(),
                server: None,
                account: None,
                rules: None,
            };
            graph
                .add_root(&place, None, None, &launched, |_| Ok(()))
                .unwrap()
        };
        let nodes = [root("a"), root("b")];
        (dir, nodes, IdleWaits::new(graph))
    }

    fn idle_entries(waits: &IdleWaits, of: NodeId) -> Vec<Entry> {
        let mut entries: Vec<Entry> = waits
            .graph
            .recent_events()
            .unwrap()
            .into_iter()
            .map(|r| r.entry)
            .filter(|e| match e {
                Entry::IdleOpened { node } | Entry::IdleEnded { node, .. } => node.id == Some(of),
                _ => false,
            })
            .collect();
        entries.reverse();
        entries
    }

    #[test]
    fn only_a_turn_hook_until_the_turn_end_counts_as_a_turn() {
        let (_dir, [node, _], waits) = waits();
        assert!(!waits.in_turn(node));
        waits.turn_started(node, "postToolUse".into());
        assert!(waits.in_turn(node));
        assert!(!waits.waiting(node));
        let _wait = waits.open(node);
        assert!(!waits.in_turn(node));
        assert!(waits.waiting(node));
        waits.turn_started(node, "postToolUse".into());
        waits.end(node, IdleEnd::Exit);
        assert!(!waits.in_turn(node));
    }

    #[test]
    fn an_ended_wait_reopens_only_when_no_newer_wait_or_turn_came() {
        let (_dir, [node, _], waits) = waits();
        let mut wait = waits.open(node);
        waits.end(node, IdleEnd::Relaunch);
        assert!(waits.reopen(&mut wait));
        assert!(!waits.reopen(&mut wait));
        waits.turn_started(node, "preToolUse".into());
        assert!(!waits.reopen(&mut wait));
    }

    #[tokio::test]
    async fn every_wait_records_its_open_and_the_cause_of_its_end() {
        let (_dir, [node, other], waits) = waits();
        let who = || Who {
            id: Some(node),
            name: "shell-1".into(),
        };
        let mut first = waits.open(node);
        let mut second = waits.open(node);
        let by = first.ended().await;
        first.finish(by);
        let mut elsewhere = waits.open(other);
        waits.turn_started(node, "beforeSubmitPrompt".into());
        let by = second.ended().await;
        second.finish(by);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), elsewhere.ended())
                .await
                .is_err()
        );
        let third = waits.open(node);
        third.finish(IdleEnd::Mail);
        let fourth = waits.open(node);
        drop(fourth);
        let mut fifth = waits.open(node);
        waits.end(node, IdleEnd::Sleep);
        let by = fifth.ended().await;
        fifth.finish(by);

        let ended = |by| Entry::IdleEnded { node: who(), by };
        let opened = || Entry::IdleOpened { node: who() };
        assert_eq!(
            idle_entries(&waits, node),
            [
                opened(),
                opened(),
                ended(IdleEnd::NewWait),
                ended(IdleEnd::Turn {
                    hook: "beforeSubmitPrompt".into()
                }),
                opened(),
                ended(IdleEnd::Mail),
                opened(),
                ended(IdleEnd::Gone),
                opened(),
                ended(IdleEnd::Sleep),
            ]
        );
    }
}
