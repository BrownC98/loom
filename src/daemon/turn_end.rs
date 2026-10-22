//! 턴 끝 검사: 사람이 띄운 노드가 글을 쓰고 `loom ask`도 `loom done`도 없이 턴을 끝내면 한 번 다시 일하게 한다.
//! 받은편지함에는 아무것도 걸지 않는다. 묻는 글인지, 쓸 수 있는 결과인지는 에이전트가 스스로 정한다.
//! 근거: docs/design.md 「사람 응답 대기」. `?`로 끝나는 문장만 보던 검사는 「어느 쪽으로 할지 알려 주세요」 같은 질문을 놓쳤다.
//! 2026-10-22 사람 결정 Q334 A: 글 검사를 지우고, 묻지도 알리지도 않은 턴 끝마다 한 번 다시 일하게 한다.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use super::App;
use crate::graph::NodeId;

/// 다시 일하게 할 때 에이전트에 넘기는 글. 할 일이 없으면 `.` 한 글자로 끝내게 해 토큰을 아낀다(2026-10-22 사람 지시 "질문아닐때의 훅 응답은 매우 짧은 텍스트로 끝내기").
const NUDGE: &str = "You ended the turn with text but without loom ask or loom done. If the text asks the human anything (a question, a choice, an approval), call loom ask \"<one line>\", then end your turn. If you finished a result the human can use, call loom done \"<what you did>\" --asked \"<the human's instruction>\". Otherwise reply only \".\" and end your turn.";

#[derive(Default)]
pub struct TurnEnds {
    /// 노드마다 이번 턴의 마지막 답 글. 턴 끝에 꺼낸다. 빈 글과 `NUDGE`가 시킨 `.`는 글이 없는 것으로 본다.
    said: Mutex<HashMap<NodeId, String>>,
    /// 바로 앞 턴 끝에 다시 일하게 한 노드. 다음 턴 끝은 검사하지 않는다(한 번만 막는다).
    nudged: Mutex<HashSet<NodeId>>,
}

impl TurnEnds {
    /// 에이전트가 답 글을 냈다. 같은 턴의 앞 글은 버린다.
    pub fn said(&self, node: NodeId, text: String) {
        self.said.lock().unwrap().insert(node, text);
    }

    /// 턴이 끝났다. 다시 일하게 할 글이 있으면 돌려준다.
    /// `unreported_root`는 노드가 사람이 띄운 노드이고, 사람 응답 대기가 없고, `loom done`을 부르지 않았는지 본다(`Graph::unreported_root`).
    pub fn ended(
        &self,
        node: NodeId,
        unreported_root: impl FnOnce() -> bool,
    ) -> Option<&'static str> {
        let said = self.said.lock().unwrap().remove(&node);
        let nudged = self.nudged.lock().unwrap().remove(&node);
        if !nudged && said.is_some_and(|text| !matches!(text.trim(), "" | ".")) && unreported_root()
        {
            self.nudged.lock().unwrap().insert(node);
            return Some(NUDGE);
        }
        None
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
    fn only_a_root_without_a_wait_or_done_is_nudged_once() {
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
        turns.said(node, " \n".into());
        assert_eq!(turns.ended(node, || true), None);
        turns.said(node, ".".into());
        assert_eq!(turns.ended(node, || true), None);
        // 물음표 없이 고르게 하는 글도 막는다.
        turns.said(node, "A안과 B안 가운데 고르시면 진행합니다.".into());
        assert_eq!(turns.ended(node, || true), Some(NUDGE));
    }
}
