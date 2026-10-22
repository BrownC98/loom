//! 턴 끝 질문 검사: 사람이 띄운 노드가 글로만 묻고 `loom ask` 없이 턴을 끝내면 한 번 다시 일하게 한다.
//! 받은편지함에는 아무것도 걸지 않는다. 에이전트가 사람에게 묻는 것인지 스스로 정한다.
//! 근거: docs/design.md 「사람 응답 대기」, 2026-10-21 사람 승인 "부모 없는 노드가 ?로 끝나는 문장을 남기고 loom ask 없이 턴을 끝내면 한 번 막고 다시 일하게 한다".
//! 완료 알림은 여기서 짐작하지 않는다. 에이전트가 `loom done --asked`로 건다(docs/design.md 「완료 알림」).
//! 사람이 일을 준 뒤 `loom done` 없이 턴을 끝내면 한 번 다시 일하게 해 잊지 않게 한다(2026-10-22 사람 지시 "이건 왜 완료 알림 안뜸", 권장안 "진행").

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use super::App;
use crate::graph::NodeId;

/// 다시 일하게 할 때 에이전트에 넘기는 글. 질문이 아니면 `.` 한 글자로 끝내게 해 토큰을 아낀다(2026-10-22 사람 지시 "질문아닐때의 훅 응답은 매우 짧은 텍스트로 끝내기").
const NUDGE: &str = "Your text ends with '?' but you did not call loom ask. A question to the human: call loom ask now, then end the turn. Not a question: reply only \".\" and end the turn.";

/// 사람이 준 일을 `loom done`으로 알리지 않고 턴을 끝냈을 때 넘기는 글. 일이 남았으면 `.`으로 끝낸다.
const DONE_NUDGE: &str = "The human gave you work and you did not call loom done. If all of it is finished, call loom done \"<what you did>\" --asked \"<the human's instruction>\" now. If work still runs (children, background tasks) or you wait for an answer, reply only \".\" and end the turn.";

/// `?` 없이 사람에게 묻는 말. 「원하시면 바로 넣겠습니다」가 `loom ask` 없이 지나갔다(2026-10-22 사람 지시 "이건 또 질문으로 안 올라오네").
const OFFERS: [&str; 6] = [
    "원하시면",
    "괜찮으시면",
    "말씀해 주시면",
    "if you want",
    "want me to",
    "let me know if",
];

#[derive(Default)]
pub struct TurnEnds {
    /// 노드마다 이번 턴의 마지막 답 글. 턴 끝에 꺼낸다.
    said: Mutex<HashMap<NodeId, String>>,
    /// 바로 앞 턴 끝에 다시 일하게 한 노드. 다음 턴 끝은 검사하지 않는다(한 번만 막는다).
    nudged: Mutex<HashSet<NodeId>>,
    /// 사람이 일을 준 뒤 `loom done`을 아직 부르지 않은 노드.
    owed: Mutex<HashSet<NodeId>>,
}

impl TurnEnds {
    /// 에이전트가 답 글을 냈다. 같은 턴의 앞 글은 버린다.
    pub fn said(&self, node: NodeId, text: String) {
        self.said.lock().unwrap().insert(node, text);
    }

    /// 턴이 끝났다. 다시 일하게 할 글이 있으면 돌려준다.
    /// `unasked_root`는 노드가 사람이 띄운 노드이고 사람 응답 대기가 없는지 본다(`Graph::unasked_root`).
    /// 이번 턴에 `loom ask`로 물었으면 대기가 남아 있다.
    pub fn ended(&self, node: NodeId, unasked_root: impl FnOnce() -> bool) -> Option<&'static str> {
        let said = self.said.lock().unwrap().remove(&node);
        let nudged = self.nudged.lock().unwrap().remove(&node);
        if nudged {
            return None;
        }
        let nudge = if said.as_deref().is_some_and(asks_in_text) {
            NUDGE
        } else if self.owed.lock().unwrap().contains(&node) {
            DONE_NUDGE
        } else {
            return None;
        };
        if !unasked_root() {
            return None;
        }
        self.nudged.lock().unwrap().insert(node);
        Some(nudge)
    }

    /// 사람이 일을 줬다(입력, 받은편지함 답).
    pub fn owe(&self, node: NodeId) {
        self.owed.lock().unwrap().insert(node);
    }

    /// 노드가 `loom done`을 불렀다.
    pub fn paid(&self, node: NodeId) {
        self.owed.lock().unwrap().remove(&node);
    }
}

impl App {
    /// 사람이 입력해 턴이 시작했다. 노드를 다시 일하는 상태로 두고, 끝나면 `loom done`을 부르게 한다.
    pub(super) fn human_input(&self, node: NodeId) -> Result<(), crate::graph::Refusal> {
        self.turn_ends.owe(node);
        self.graph.human_input(node)
    }

    /// 턴이 끝났다. 다시 일하게 할 글이 있으면 돌려준다.
    pub(super) fn turn_end(&self, node: NodeId) -> Option<&'static str> {
        self.turn_ends.ended(node, || {
            self.graph.unasked_root(node).unwrap_or_else(|e| {
                eprintln!("loom: 노드 {}의 턴 끝을 검사하지 못했다: {e}", node.0);
                false
            })
        })
    }
}

/// 글에 물음표로 끝나는 문장이 있다. 코드 블록, 인라인 코드, 인용 줄(`>`)은 보지 않는다.
fn asks_in_text(text: &str) -> bool {
    let mut fenced = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced || line.starts_with('>') {
            continue;
        }
        let prose: String = line.split('`').step_by(2).collect::<Vec<_>>().join(" ");
        let lower = prose.to_lowercase();
        if OFFERS.iter().any(|offer| lower.contains(offer)) {
            return true;
        }
        if prose
            .split_whitespace()
            .map(|word| word.trim_end_matches(['*', '_', ')', '"', '\'', '」', '』']))
            .any(|word| word.ends_with(['?', '？']))
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sentence_ending_with_a_question_mark_asks() {
        assert!(asks_in_text("끝냈다.\n병합할까요?"));
        assert!(asks_in_text("병합할까요？ 아니면 기다릴까요."));
        assert!(asks_in_text("**Merge now?**"));
        assert!(asks_in_text("Shall I merge? I can wait."));
        assert!(asks_in_text("원하시면 바로 넣겠습니다."));
        assert!(asks_in_text("Want me to merge it."));
    }

    #[test]
    fn code_and_quotes_do_not_ask() {
        assert!(!asks_in_text("끝냈다."));
        assert!(!asks_in_text("```\nlet x = a?;\nwhy?\n```\n끝냈다."));
        assert!(!asks_in_text("`a?` 연산자를 썼다."));
        assert!(!asks_in_text("> 사람: 왜 그런가?\n원인을 고쳤다."));
        assert!(!asks_in_text("URL은 http://x/?a=1 이다."));
    }

    #[test]
    fn only_a_root_without_a_wait_is_nudged_once() {
        let turns = TurnEnds::default();
        let node = NodeId(1);
        turns.said(node, "병합할까요?".into());
        assert_eq!(turns.ended(node, || false), None, "child or asked");
        turns.said(node, "병합할까요?".into());
        assert_eq!(turns.ended(node, || true), Some(NUDGE));
        // 다시 일한 턴이 또 묻고 끝나도 막지 않는다.
        turns.said(node, "병합할까요?".into());
        assert_eq!(turns.ended(node, || true), None);
        turns.said(node, "병합할까요?".into());
        assert_eq!(turns.ended(node, || true), Some(NUDGE));
        // 답 글이 없는 턴 끝은 막지 않는다. 그래도 한 번 막은 표시는 지운다.
        assert_eq!(turns.ended(node, || true), None);
        turns.said(node, "끝냈다.".into());
        assert_eq!(turns.ended(node, || true), None);
        turns.said(node, "병합할까요?".into());
        assert_eq!(turns.ended(node, || true), Some(NUDGE));
    }

    #[test]
    fn work_from_the_human_is_nudged_until_done() {
        let turns = TurnEnds::default();
        let node = NodeId(1);
        turns.said(node, "고쳤다.".into());
        assert_eq!(turns.ended(node, || true), None, "no work from the human");
        turns.owe(node);
        turns.said(node, "고쳤다.".into());
        assert_eq!(
            turns.ended(node, || false),
            None,
            "child or waiting for the human"
        );
        assert_eq!(turns.ended(node, || true), Some(DONE_NUDGE));
        // 「.」로 끝낸 다음 턴은 막지 않는다. 그 뒤 턴 끝(자식 보고 등)에는 다시 묻는다.
        turns.said(node, ".".into());
        assert_eq!(turns.ended(node, || true), None);
        assert_eq!(turns.ended(node, || true), Some(DONE_NUDGE));
        turns.paid(node);
        assert_eq!(turns.ended(node, || true), None);
        assert_eq!(turns.ended(node, || true), None);
    }
}
