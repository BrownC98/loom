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
/// 검사 이름은 붙이지 않고 지금 하는 일이나 상황을 짧게 적는다(같은 날 "턴 끝 명시하지 말고, 그냥 현재 너의 작업 또는 상황을 브리핑하는 짧은 단어로 명해").
const NUDGE: &str = "You ended the turn with text but without loom ask or loom done. If the text asks the human anything (a question, a choice, an approval), call loom ask \"<one line>\", then end your turn. If you finished a result the human can use, call loom done \"<what you did>\" --asked \"<the human's instruction>\". Otherwise reply with only a few words in the language the human uses with you that brief your current work or state. Do not name this check. Example in Korean: \"코디 결과 대기\". Then end your turn.";

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

    /// 턴이 끝났다. 이번 턴의 답 글과, 다시 일하게 할 글이 있으면 그 글을 돌려준다.
    /// `unreported_root`는 노드가 사람이 띄운 노드이고, 사람 응답 대기가 없고, `loom done`을 부르지 않았는지 본다(`Graph::unreported_root`).
    pub fn ended(&self, node: NodeId, unreported_root: impl FnOnce() -> bool) -> TurnEnd {
        let reply = self.said.lock().unwrap().remove(&node);
        let human = self.human.lock().unwrap().remove(&node);
        let nudge = (human
            && reply.as_ref().is_some_and(|text| !text.trim().is_empty())
            && unreported_root())
        .then_some(NUDGE);
        TurnEnd { reply, nudge }
    }
}

/// 끝난 턴. `reply`는 심플 카드의 상태 줄로 남긴다.
pub struct TurnEnd {
    reply: Option<String>,
    nudge: Option<&'static str>,
}

impl App {
    /// 턴이 끝났다. 다시 일하게 할 글이 있으면 돌려준다.
    pub(super) fn turn_end(&self, node: NodeId) -> Option<&'static str> {
        let TurnEnd { reply, nudge } = self.turn_ends.ended(node, || {
            self.graph.unreported_root(node).unwrap_or_else(|e| {
                eprintln!("loom: 노드 {}의 턴 끝을 검사하지 못했다: {e}", node.0);
                false
            })
        });
        if let Some(text) = reply
            && let Err(e) = self.graph.set_reply(node, &text)
        {
            eprintln!("loom: 노드 {}의 답 글을 남기지 못했다: {e}", node.0);
        }
        // 다시 띄울 노드다. 다시 일하게 한 턴은 다시 띄우기가 끊거나 늦춘다.
        nudge.filter(|_| !self.relaunches.holds_mail(node))
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
        assert_eq!(turns.ended(node, || true).nudge, None, "not a human turn");
        turns.human(node);
        turns.said(node, "병합할까요?".into());
        assert_eq!(
            turns.ended(node, || false).nudge,
            None,
            "child, asked or done"
        );
        turns.human(node);
        turns.said(node, "A안과 B안 가운데 고르시면 진행합니다.".into());
        assert_eq!(turns.ended(node, || true).nudge, Some(NUDGE));
        // 다시 일한 턴은 사람 입력이 없다. 답 글은 상태 줄로 돌려준다.
        turns.said(node, "코디 결과 대기".into());
        let end = turns.ended(node, || true);
        assert_eq!(
            (end.reply.as_deref(), end.nudge),
            (Some("코디 결과 대기"), None)
        );
        // 답 글이 없는 사람 턴은 막지 않는다.
        turns.human(node);
        turns.said(node, " \n".into());
        assert_eq!(turns.ended(node, || true).nudge, None);
    }
}
