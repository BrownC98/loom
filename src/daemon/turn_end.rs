//! 턴 끝 검사: 사람이 띄운 노드가 사람 입력을 받은 턴에 글을 쓰고 `loom ask`도 `loom done`도 없이 끝내면 한 번 다시 일하게 한다.
//! 사람 입력이 시작하지 않은 턴(백그라운드 작업 알림, 자식 메시지)은 보지 않는다. 그런 턴마다 깨워 짧은 답 턴이 끝없이 붙었다
//! (2026-10-22 사람 지시 "이거 뜨는 근본 원인 해결하라").
//! 받은편지함에는 아무것도 걸지 않는다. 묻는 글인지, 쓸 수 있는 결과인지는 에이전트가 스스로 정한다.
//! 근거: docs/design.md 「사람 응답 대기」. `?`로 끝나는 문장만 보던 검사는 「어느 쪽으로 할지 알려 주세요」 같은 질문을 놓쳤다.
//! 2026-10-22 사람 결정 Q334 A: 글 검사를 지우고, 묻지도 알리지도 않은 턴 끝마다 한 번 다시 일하게 한다.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use super::App;
use crate::graph::NodeId;

/// 다시 일하게 할 때 에이전트에 넘기는 글. 할 일이 없으면 왜 없는지 사람의 말로 한두 낱말만 답하게 해 토큰을 아낀다
/// (2026-10-22 사람 지시 "질문아닐때의 훅 응답은 매우 짧은 텍스트로 끝내기", "훅 짧은 응답에 대해 . 대신 이 응답을 왜 하는지 짧은 한두단어로 표기하라 (사용자 언어)").
/// 답 앞에 이 검사의 이름을 붙여 어떤 훅에 왜 답하는지 보이게 한다(같은 날 "대체 문구는 어떤 훅에 대해 어떤 상황이라 이 문구가 나오는지 설명되도록 명시해줘").
const NUDGE: &str = "You ended the turn with text but without loom ask or loom done. If the text asks the human anything (a question, a choice, an approval), call loom ask \"<one line>\", then end your turn. If you finished a result the human can use, call loom done \"<what you did>\" --asked \"<the human's instruction>\". Otherwise reply with only one line in the language the human uses with you: the name of this check (the loom turn-end check), a colon, and one or two words that say why no ask or done is needed (for example a status report or a wait). Example in Korean: \"loom 턴 끝 확인: 진행 보고\". Then end your turn.";

#[derive(Default)]
pub struct TurnEnds {
    /// 노드마다 이번 턴의 마지막 답 글. 턴 끝에 꺼낸다. 빈 글은 글이 없는 것으로 본다.
    said: Mutex<HashMap<NodeId, String>>,
    /// 사람 입력(TUI 입력, 받은편지함 답)을 받고 아직 턴 끝을 검사하지 않은 노드. 턴 끝에 지운다. 다시 일한 턴은 사람 입력이 없으므로 검사하지 않는다.
    human: Mutex<HashSet<NodeId>>,
}

impl TurnEnds {
    /// 에이전트가 답 글을 냈다. 같은 턴의 앞 글은 버린다.
    pub fn said(&self, node: NodeId, text: String) {
        self.said.lock().unwrap().insert(node, text);
    }

    /// 사람이 노드에 입력했다. 그 입력을 받은 턴의 끝을 검사한다.
    pub fn human(&self, node: NodeId) {
        self.human.lock().unwrap().insert(node);
    }

    /// 턴이 끝났다. 다시 일하게 할 글이 있으면 돌려준다.
    /// `unreported_root`는 노드가 사람이 띄운 노드이고, 사람 응답 대기가 없고, `loom done`을 부르지 않았는지 본다(`Graph::unreported_root`).
    pub fn ended(
        &self,
        node: NodeId,
        unreported_root: impl FnOnce() -> bool,
    ) -> Option<&'static str> {
        let said = self.said.lock().unwrap().remove(&node);
        let human = self.human.lock().unwrap().remove(&node);
        (human && said.is_some_and(|text| !text.trim().is_empty()) && unreported_root())
            .then_some(NUDGE)
    }
}

impl App {
    /// 턴이 끝났다. 다시 일하게 할 글이 있으면 돌려준다.
    pub(super) fn turn_end(&self, node: NodeId) -> Option<&'static str> {
        self.turn_ends.ended(node, || {
            self.graph.unreported_root(node).unwrap_or_else(|e| {
                eprintln!("loom: 노드 {}의 턴 끝을 검사하지 못했다: {e}", node.0);
                false
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_human_turn_of_an_unreported_root_is_nudged_once() {
        let turns = TurnEnds::default();
        let node = NodeId(1);
        // 알림·자식 메시지가 시작한 턴은 보지 않는다.
        turns.said(node, "병합했습니다".into());
        assert_eq!(turns.ended(node, || true), None, "not a human turn");
        turns.human(node);
        turns.said(node, "병합할까요?".into());
        assert_eq!(turns.ended(node, || false), None, "child, asked or done");
        turns.human(node);
        turns.said(node, "A안과 B안 가운데 고르시면 진행합니다.".into());
        assert_eq!(turns.ended(node, || true), Some(NUDGE));
        // 다시 일한 턴은 사람 입력이 없다.
        turns.said(node, "진행 상황".into());
        assert_eq!(turns.ended(node, || true), None);
        // 답 글이 없는 사람 턴은 막지 않는다.
        turns.human(node);
        turns.said(node, " \n".into());
        assert_eq!(turns.ended(node, || true), None);
    }
}
