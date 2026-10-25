use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use tokio::sync::oneshot;

use super::TurnId;
use crate::graph::{Entry, Graph, IdleEnd, NodeId, Who};

pub struct IdleWaits {
    graph: Arc<Graph>,
    ends: Mutex<HashMap<NodeId, oneshot::Sender<IdleEnd>>>,
    turns: Mutex<HashSet<NodeId>>,
    watches: Mutex<HashMap<NodeId, Watch>>,
    next_said: AtomicU64,
}

#[derive(Default)]
struct Watch {
    heard: bool,
    drafted: bool,
    typed_at: Option<SystemTime>,
    turn: Option<TurnId>,
    past: HashSet<TurnId>,
    said: Option<(Said, TurnId)>,
}

impl Watch {
    fn enter(&mut self, turn: TurnId) {
        if self.past.contains(&turn) {
            return;
        }
        if let Some((_, armed)) = self.said.take_if(|(_, armed)| *armed != turn) {
            self.past.insert(armed);
        }
        if self.turn.as_ref() != Some(&turn)
            && let Some(old) = self.turn.replace(turn)
        {
            self.past.insert(old);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Said(u64);

#[derive(PartialEq, Debug)]
pub enum Expiry {
    Answered,
    Listening,
    Missed,
}

pub struct Wait<'a> {
    waits: &'a IdleWaits,
    node: NodeId,
    who: Option<Who>,
    end: oneshot::Receiver<IdleEnd>,
}

impl IdleWaits {
    pub fn new(graph: Arc<Graph>) -> Self {
        Self {
            graph,
            ends: Mutex::default(),
            turns: Mutex::default(),
            watches: Mutex::default(),
            next_said: AtomicU64::new(0),
        }
    }

    pub fn open(&self, node: NodeId) -> Wait<'_> {
        self.turns.lock().unwrap().remove(&node);
        self.watch(node, |w| w.heard = true);
        let (tx, end) = oneshot::channel();
        if let Some(old) = self.ends.lock().unwrap().insert(node, tx) {
            let _ = old.send(IdleEnd::NewWait);
        }
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
            who,
            end,
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
        self.ends
            .lock()
            .unwrap()
            .get(&node)
            .is_some_and(|end| !end.is_closed())
    }

    pub fn typed_since(&self, node: NodeId, since: SystemTime) -> bool {
        self.watches
            .lock()
            .unwrap()
            .get(&node)
            .and_then(|w| w.typed_at)
            .is_some_and(|at| at >= since)
    }

    pub fn in_turn(&self, node: NodeId) -> bool {
        self.turns.lock().unwrap().contains(&node)
    }

    pub fn turn_started(&self, node: NodeId, hook: String) {
        self.end_wait(node, IdleEnd::Turn { hook });
        self.turns.lock().unwrap().insert(node);
    }

    pub fn prompted(&self, node: NodeId, turn: TurnId) {
        self.watch(node, |w| {
            w.drafted = false;
            w.enter(turn);
        });
    }

    pub fn stepped(&self, node: NodeId, turn: TurnId) {
        self.watch(node, |w| w.enter(turn));
    }

    pub fn end(&self, node: NodeId, by: IdleEnd) {
        self.watches.lock().unwrap().remove(&node);
        self.end_wait(node, by);
    }

    pub fn release(&self, node: NodeId) {
        if let Some(end) = self.ends.lock().unwrap().remove(&node) {
            let _ = end.send(IdleEnd::Relaunch);
        }
    }

    fn end_wait(&self, node: NodeId, by: IdleEnd) {
        self.turns.lock().unwrap().remove(&node);
        if let Some(old) = self.ends.lock().unwrap().remove(&node) {
            let _ = old.send(by);
        }
    }

    pub fn said(&self, node: NodeId, turn: TurnId) -> Option<Said> {
        let mut watches = self.watches.lock().unwrap();
        let w = watches.get_mut(&node)?;
        let follows = w
            .turn
            .as_ref()
            .is_none_or(|now| *now == turn || w.past.contains(now));
        if !w.heard || w.drafted || w.past.contains(&turn) || !follows {
            return None;
        }
        w.enter(turn.clone());
        let said = Said(self.next_said.fetch_add(1, Ordering::Relaxed));
        w.said = Some((said, turn));
        Some(said)
    }

    pub fn stop_reached(&self, node: NodeId, turn: Option<TurnId>) {
        self.watch(node, |w| match turn {
            Some(turn) => {
                w.said.take_if(|(_, armed)| *armed == turn);
                w.past.insert(turn);
            }
            None => w.said = None,
        });
    }

    pub fn typed(&self, node: NodeId) {
        self.typed_at(node, SystemTime::now());
    }

    pub(super) fn typed_at(&self, node: NodeId, at: SystemTime) {
        self.watch(node, |w| {
            w.drafted = true;
            w.typed_at = Some(at);
            if let Some((_, armed)) = w.said.take() {
                w.past.insert(armed);
            }
        });
    }

    pub fn expire(&self, node: NodeId, said: Said) -> Expiry {
        let mut watches = self.watches.lock().unwrap();
        let Some(w) = watches
            .get_mut(&node)
            .filter(|w| w.said.as_ref().is_some_and(|(armed, _)| *armed == said))
        else {
            return Expiry::Answered;
        };
        if self.waiting(node) {
            return Expiry::Listening;
        }
        w.said = None;
        w.heard = false;
        self.turns.lock().unwrap().remove(&node);
        Expiry::Missed
    }

    fn watch(&self, node: NodeId, change: impl FnOnce(&mut Watch)) {
        change(self.watches.lock().unwrap().entry(node).or_default());
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

    fn record(&mut self, by: IdleEnd) {
        if let Some(node) = self.who.take() {
            self.waits.record(Entry::IdleEnded { node, by });
        }
    }
}

impl Drop for Wait<'_> {
    fn drop(&mut self) {
        self.record(IdleEnd::Gone);
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
        let graph = Arc::new(Graph::open(db, crate::profile::linked).unwrap());
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
        let gone = waits.open(node);
        drop(gone);
        assert!(!waits.waiting(node), "the hook that held the wait is gone");
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

    #[test]
    fn a_turn_misses_its_wait_once_per_process_and_only_while_nothing_answered() {
        let (_dir, [node, _], waits) = waits();
        let turn = |id: &str| TurnId(id.into());
        let prompt = |id: &str| {
            waits.turn_started(node, "beforeSubmitPrompt".into());
            waits.prompted(node, turn(id));
        };
        assert_eq!(
            waits.said(node, turn("1")),
            None,
            "no stop hook has reached loom yet"
        );
        waits.open(node).finish(IdleEnd::Mail);
        prompt("1");
        let said = waits.said(node, turn("1")).unwrap();
        let missed = waits.said(node, turn("1")).unwrap();
        assert_eq!(
            waits.expire(node, said),
            Expiry::Answered,
            "a newer turn end came"
        );
        assert_eq!(waits.expire(node, missed), Expiry::Missed);
        assert!(!waits.in_turn(node));
        assert_eq!(waits.expire(node, missed), Expiry::Answered);
        assert_eq!(
            waits.said(node, turn("1")),
            None,
            "the process proves its stop hook again"
        );

        waits.open(node).finish(IdleEnd::Mail);
        waits.end(node, IdleEnd::Relaunch);
        assert_eq!(
            waits.said(node, turn("2")),
            None,
            "a new process proves its stop hook again"
        );

        waits.open(node).finish(IdleEnd::Mail);
        prompt("3");
        waits.stop_reached(node, Some(turn("3")));
        assert_eq!(
            waits.said(node, turn("3")),
            None,
            "the stop hook of that turn came first"
        );
        prompt("4");
        prompt("5");
        assert_eq!(waits.said(node, turn("4")), None, "a newer turn started");
        assert!(waits.in_turn(node));

        let said = waits.said(node, turn("5")).unwrap();
        let wait = waits.open(node);
        assert_eq!(
            waits.expire(node, said),
            Expiry::Listening,
            "the wait opened"
        );
        wait.finish(IdleEnd::Mail);

        prompt("7");
        let said = waits.said(node, turn("7")).unwrap();
        waits.stop_reached(node, None);
        assert_eq!(
            waits.expire(node, said),
            Expiry::Answered,
            "the stop hook reached loom"
        );
        let said = waits.said(node, turn("7")).unwrap();
        waits.typed(node);
        assert_eq!(
            waits.expire(node, said),
            Expiry::Answered,
            "the human types"
        );
        assert_eq!(
            waits.said(node, turn("7")),
            None,
            "the human typed in that turn"
        );
        prompt("8");
        assert!(waits.said(node, turn("8")).is_some());

        waits.end(node, IdleEnd::Relaunch);
        let wait = waits.open(node);
        let said = waits.said(node, turn("9")).unwrap();
        assert_eq!(
            waits.expire(node, said),
            Expiry::Listening,
            "a wait is open"
        );
        wait.finish(IdleEnd::Mail);
        assert_eq!(
            waits.expire(node, said),
            Expiry::Missed,
            "the finished wait no longer listens"
        );
    }

    #[test]
    fn the_event_log_keeps_shown_events_when_waits_fill_the_newest_rows() {
        let (_dir, [node, _], waits) = waits();
        for _ in 0..260 {
            waits.open(node).finish(IdleEnd::Mail);
        }
        let log = waits.graph.event_log().unwrap();
        assert!(!log.records.is_empty());
        assert!(
            log.records
                .iter()
                .all(|r| !matches!(r.entry, Entry::IdleOpened { .. } | Entry::IdleEnded { .. }))
        );
        assert_eq!(log.last, waits.graph.recent_events().unwrap()[0].id);
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
        let sixth = waits.open(node);
        waits.graph.remove(node).unwrap();
        drop(sixth);

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
                opened(),
                ended(IdleEnd::Gone),
            ]
        );
    }
}
