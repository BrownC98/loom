use std::sync::Arc;

use super::App;
use crate::graph::{Entry, Graph, NodeId, TurnCheck, TurnMark};

const NUDGE: &str = "You ended the turn with text but without loom ask or loom done. If the text asks the human anything (a question, a choice, an approval), call loom ask \"<one line>\", then end your turn. If you finished a result the human can use, call loom done \"<what you did>\" --asked \"<the human's instruction>\". Otherwise reply with only a few words in the language the human uses with you that brief your current work or state. Do not name this check. Example in Korean: \"코디 결과 대기\". Then end your turn.";

const KIND_NUDGE: &str = "Your last reply has a line that starts with a kind word (QUESTION, ANSWER, STATUS, DONE, READY, MERGED, USER DIRECTION, ...), but you called no loom verb in this turn. Text in the terminal reaches nobody. If you meant a message, send it now with its verb: loom ask, loom answer, loom done, loom note, or a gate verb (see loom --help). Otherwise reply with a few words without that line. Then end your turn.";

const QUESTION_BOX_NUDGE: &str = "You used your AskQuestion tool in this turn. The human answers only loom ask, and loom keeps no record of that box or its answer. Ask the same question again with loom ask in the one-line form of the ask-human rules, then end your turn. Do not use AskQuestion again.";

pub struct TurnEnds {
    graph: Arc<Graph>,
}

impl TurnEnds {
    pub fn new(graph: Arc<Graph>) -> Self {
        TurnEnds { graph }
    }

    fn mark(&self, node: NodeId, mark: TurnMark) {
        if let Err(e) = self.graph.mark_turn(node, mark) {
            eprintln!("loom: 노드 {}의 턴 표시를 쓰지 못했다: {e}", node.0);
        }
    }

    pub fn said(&self, node: NodeId, text: &str, turn: Option<&str>) {
        if let Err(e) = self.graph.said(node, text, turn) {
            eprintln!("loom: 노드 {}의 답 글을 남기지 못했다: {e}", node.0);
        }
    }

    pub fn stepped(&self, node: NodeId, turn: Option<&str>) {
        if let Some(turn) = turn
            && let Err(e) = self.graph.stepped(node, turn)
        {
            eprintln!("loom: 노드 {}의 턴을 쓰지 못했다: {e}", node.0);
        }
    }

    pub fn stopped(&self, node: NodeId, turn: Option<&str>) {
        if let Some(turn) = turn
            && let Err(e) = self.graph.stopped(node, turn)
        {
            eprintln!("loom: 노드 {}의 끝난 턴을 쓰지 못했다: {e}", node.0);
        }
    }

    pub fn human(&self, node: NodeId) {
        self.mark(node, TurnMark::Human);
    }

    pub fn called(&self, node: NodeId) {
        self.mark(node, TurnMark::Called);
    }

    pub fn question_box(&self, node: NodeId) {
        self.mark(node, TurnMark::QuestionBox);
    }

    pub fn asked(&self, node: NodeId) {
        self.mark(node, TurnMark::Asked);
    }

    pub fn launched(&self, node: NodeId) {
        if let Err(e) = self.graph.turn_launched(node) {
            eprintln!("loom: 노드 {}의 턴 표시를 지우지 못했다: {e}", node.0);
        }
    }
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
        let gated_repo = self.gated_repo(node).unwrap_or_else(|e| {
            eprintln!("loom: 노드 {}의 저장소를 보지 못했다: {e}", node.0);
            false
        });
        let nudge = self.graph.end_turn(node, gated_repo)?;
        let Some(check) = nudge.filter(|_| !self.relaunch_planned(node)) else {
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
