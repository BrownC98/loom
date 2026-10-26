use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::App;
use crate::graph::{Entry, Graph, NodeId, TurnCheck};

const HUMAN: u8 = 1;
const CALLED: u8 = 2;
const KIND_NUDGED: u8 = 4;
const ASKED: u8 = 8;

const NUDGE: &str = "You ended the turn with text but without loom ask or loom done. If the text asks the human anything (a question, a choice, an approval), call loom ask \"<one line>\", then end your turn. If you finished a result the human can use, call loom done \"<what you did>\" --asked \"<the human's instruction>\". Otherwise reply with only a few words in the language the human uses with you that brief your current work or state. Do not name this check. Example in Korean: \"코디 결과 대기\". Then end your turn.";

const KIND_NUDGE: &str = "Your last reply has a line that starts with a kind word (QUESTION, ANSWER, STATUS, DONE, READY, MERGED, USER DIRECTION, ...), but you called no loom verb in this turn. Text in the terminal reaches nobody. If you meant a message, send it now with its verb: loom ask, loom answer, loom done, loom note, or a gate verb (see loom --help). Otherwise reply with a few words without that line. Then end your turn.";

const QUESTION_BOX_NUDGE: &str = "You used your AskQuestion tool in this turn. The human answers only loom ask, and loom keeps no record of that box or its answer. Ask the same question again with loom ask in the one-line form of the ask-human rules, then end your turn. Do not use AskQuestion again.";

pub struct TurnEnds {
    said: Mutex<HashMap<NodeId, String>>,
    said_at: Mutex<HashMap<NodeId, Instant>>,
    turn: Mutex<HashMap<NodeId, String>>,
    ended_turns: Mutex<HashMap<NodeId, HashSet<String>>>,
    question_box: Mutex<HashSet<NodeId>>,
    graph: Arc<Graph>,
}

impl TurnEnds {
    pub fn new(graph: Arc<Graph>) -> Self {
        TurnEnds {
            said: Mutex::default(),
            said_at: Mutex::default(),
            turn: Mutex::default(),
            ended_turns: Mutex::default(),
            question_box: Mutex::default(),
            graph,
        }
    }

    fn mark(&self, node: NodeId, bit: u8) {
        if let Err(e) = self.graph.add_turn_flag(node, bit) {
            eprintln!("loom: 노드 {}의 턴 표시를 쓰지 못했다: {e}", node.0);
        }
    }

    pub fn said(&self, node: NodeId, text: String, turn: Option<&str>) {
        self.said_at(node, text, turn, Instant::now());
    }

    pub(super) fn said_at(&self, node: NodeId, text: String, turn: Option<&str>, at: Instant) {
        let current = self.turn.lock().unwrap();
        let ended = self.ended_turns.lock().unwrap();
        if turn.is_some_and(|said| {
            current.get(&node).is_some_and(|now| now != said)
                || ended.get(&node).is_some_and(|done| done.contains(said))
        }) {
            return;
        }
        self.said.lock().unwrap().insert(node, text);
        self.said_at.lock().unwrap().insert(node, at);
    }

    pub fn stepped(&self, node: NodeId, turn: Option<&str>) {
        let mut current = self.turn.lock().unwrap();
        self.said_at.lock().unwrap().remove(&node);
        if let Some(turn) = turn {
            current.insert(node, turn.to_owned());
        }
    }

    pub fn stopped(&self, node: NodeId, turn: Option<&str>) {
        if let Some(turn) = turn {
            let current = self.turn.lock().unwrap();
            let mut ended = self.ended_turns.lock().unwrap();
            ended.entry(node).or_default().insert(turn.to_owned());
            drop(current);
        }
    }

    pub fn reply_age(&self, node: NodeId) -> Option<Duration> {
        self.said_at
            .lock()
            .unwrap()
            .get(&node)
            .map(Instant::elapsed)
    }

    pub fn human(&self, node: NodeId) {
        self.mark(node, HUMAN);
    }

    pub fn called(&self, node: NodeId) {
        self.mark(node, CALLED);
    }

    pub fn question_box(&self, node: NodeId) {
        self.question_box.lock().unwrap().insert(node);
    }

    pub fn asked(&self, node: NodeId) {
        self.mark(node, ASKED);
    }

    pub fn launched(&self, node: NodeId) {
        let mut current = self.turn.lock().unwrap();
        current.remove(&node);
        self.ended_turns.lock().unwrap().remove(&node);
        drop(current);
        if let Err(e) = self.graph.clear_turn_flag(node, ASKED) {
            eprintln!("loom: 노드 {}의 턴 표시를 지우지 못했다: {e}", node.0);
        }
        self.question_box.lock().unwrap().remove(&node);
    }

    pub fn ended(
        &self,
        node: NodeId,
        gated_repo: impl FnOnce() -> bool,
        unreported_root: impl FnOnce() -> bool,
    ) -> rusqlite::Result<TurnEnd> {
        let flags = self.graph.take_turn_flags(node)?;
        let reply = self.said.lock().unwrap().remove(&node);
        self.said_at.lock().unwrap().remove(&node);
        let mut current = self.turn.lock().unwrap();
        if let Some(turn) = current.remove(&node) {
            let mut ended = self.ended_turns.lock().unwrap();
            ended.entry(node).or_default().insert(turn);
        }
        drop(current);
        let (human, called, nudged, asked) = (
            flags & HUMAN != 0,
            flags & CALLED != 0,
            flags & KIND_NUDGED != 0,
            flags & ASKED != 0,
        );
        if self.question_box.lock().unwrap().remove(&node) && !asked {
            return Ok(TurnEnd {
                reply,
                nudge: Some(TurnCheck::QuestionBox),
            });
        }
        let mut gated = None;
        let mut gated_repo = Some(gated_repo);
        let kind_line = !called
            && !nudged
            && reply.as_deref().is_some_and(|text| {
                crate::protocol::kind_line(text, |found| {
                    !found.gated_only
                        || *gated.get_or_insert_with(|| gated_repo.take().is_some_and(|f| f()))
                })
                .is_some()
            });
        let nudge = if kind_line {
            self.mark(node, KIND_NUDGED);
            Some(TurnCheck::KindLine)
        } else {
            (human
                && reply.as_ref().is_some_and(|text| !text.trim().is_empty())
                && unreported_root())
            .then_some(TurnCheck::Unreported)
        };
        Ok(TurnEnd { reply, nudge })
    }
}

pub struct TurnEnd {
    reply: Option<String>,
    nudge: Option<TurnCheck>,
}

fn nudge_text(check: TurnCheck) -> &'static str {
    match check {
        TurnCheck::Unreported => NUDGE,
        TurnCheck::KindLine => KIND_NUDGE,
        TurnCheck::QuestionBox => QUESTION_BOX_NUDGE,
    }
}

impl App {
    pub(super) fn turn_end(&self, node: NodeId) -> rusqlite::Result<Option<&'static str>> {
        let gated_repo = || {
            self.gated_repo(node).unwrap_or_else(|e| {
                eprintln!("loom: 노드 {}의 저장소를 보지 못했다: {e}", node.0);
                false
            })
        };
        let TurnEnd { reply, nudge } = self.turn_ends.ended(node, gated_repo, || {
            self.graph.unreported_root(node).unwrap_or_else(|e| {
                eprintln!("loom: 노드 {}의 턴 끝을 검사하지 못했다: {e}", node.0);
                false
            })
        })?;
        if let Some(text) = reply
            && let Err(e) = self.graph.set_reply(node, &text)
        {
            eprintln!("loom: 노드 {}의 답 글을 남기지 못했다: {e}", node.0);
        }
        let Some(check) = nudge.filter(|_| !self.holds_mail(node)) else {
            return Ok(None);
        };
        if let Err(e) = self
            .graph
            .record_about(node, |node| Entry::Nudged { node, check })
        {
            eprintln!("loom: 노드 {}의 턴 끝 검사를 남기지 못했다: {e}", node.0);
        }
        Ok(Some(nudge_text(check)))
    }
}
