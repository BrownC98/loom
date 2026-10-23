//! 턴 끝 검사: 사람이 띄운 노드가 사람 입력을 받은 턴에 글을 쓰고 `loom ask`도 `loom done`도 없이 끝내면 한 번 다시 일하게 한다.
//! 사람 입력이 시작하지 않은 턴(백그라운드 작업 알림, 자식 메시지)은 보지 않는다. 그런 턴마다 깨워 짧은 답 턴이 끝없이 붙었다
//! (2026-10-22 사람 지시 "이거 뜨는 근본 원인 해결하라").
//! 받은편지함에는 아무것도 걸지 않는다. 묻는 글인지, 쓸 수 있는 결과인지는 에이전트가 스스로 정한다.
//! 모든 노드: loom 명령 없이 끝난 턴의 마지막 답이 종류 낱말(QUESTION, READY 등)로 시작하는 줄을 가지면 한 번 다시 일하게 한다(출력 표준 2단계).
//! 근거: docs/design.md 「사람 응답 대기」·「메시지 종류」. `?`로 끝나는 문장만 보던 검사는 「어느 쪽으로 할지 알려 주세요」 같은 질문을 놓쳤다.
//! 2026-10-22 사람 결정 Q334 A: 글 검사를 지우고, 묻지도 알리지도 않은 턴 끝마다 한 번 다시 일하게 한다.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use super::App;
use crate::graph::NodeId;

/// 다시 일하게 할 때 에이전트에 넘기는 글. 할 일이 없으면 왜 없는지 사람의 말로 한두 낱말만 답하게 해 토큰을 아낀다
/// (2026-10-22 사람 지시 "질문아닐때의 훅 응답은 매우 짧은 텍스트로 끝내기", "훅 짧은 응답에 대해 . 대신 이 응답을 왜 하는지 짧은 한두단어로 표기하라 (사용자 언어)").
/// 검사 이름은 붙이지 않고 지금 하는 일이나 상황을 짧게 적는다(같은 날 "턴 끝 명시하지 말고, 그냥 현재 너의 작업 또는 상황을 브리핑하는 짧은 단어로 명해").
const NUDGE: &str = "You ended the turn with text but without loom ask or loom done. If the text asks the human anything (a question, a choice, an approval), call loom ask \"<one line>\", then end your turn. If you finished a result the human can use, call loom done \"<what you did>\" --asked \"<the human's instruction>\". Otherwise reply with only a few words in the language the human uses with you that brief your current work or state. Do not name this check. Example in Korean: \"코디 결과 대기\". Then end your turn.";

/// 종류 낱말 줄을 터미널에만 쓰고 loom 동사를 부르지 않은 턴 끝. 모든 노드를 본다.
/// 근거: 출력 표준 R8 v5 ⑤ 4층(받는 곳이 있는 출력은 loom이 받는 곳을 고르는 명령으로만 낸다).
const KIND_NUDGE: &str = "Your last reply has a line that starts with a kind word (QUESTION, ANSWER, STATUS, DONE, READY, MERGED, USER DIRECTION, ...), but you called no loom verb in this turn. Text in the terminal reaches nobody. If you meant a message, send it now with its verb: loom ask, loom answer, loom done, loom note, or a gate verb (see loom --help). Otherwise reply with a few words without that line. Then end your turn.";

#[derive(Default)]
pub struct TurnEnds {
    /// 노드마다 이번 턴의 마지막 답 글. 턴 끝에 꺼낸다. 빈 글은 글이 없는 것으로 본다.
    said: Mutex<HashMap<NodeId, String>>,
    /// 사람 입력(TUI 입력, 받은편지함 답)을 받고 아직 턴 끝을 검사하지 않은 노드. 턴 끝에 지운다. 다시 일한 턴은 사람 입력이 없으므로 검사하지 않는다.
    human: Mutex<HashSet<NodeId>>,
    /// 이번 턴에 loom 메시지 동사(ask·answer·done·note·관문 동사)를 부른 노드. 턴 끝에 지운다.
    called: Mutex<HashSet<NodeId>>,
    /// 종류 낱말 줄 때문에 다시 일하게 한 노드. 다음 턴 끝에는 그 검사를 하지 않는다. 같은 글을 고집하면 끝없이 깨우지 않는다.
    kind_nudged: Mutex<HashSet<NodeId>>,
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

    /// 노드가 이번 턴에 loom 메시지 동사를 불렀다.
    pub fn called(&self, node: NodeId) {
        self.called.lock().unwrap().insert(node);
    }

    /// 턴이 끝났다. 이번 턴의 답 글과, 다시 일하게 할 글이 있으면 그 글을 돌려준다.
    /// `gated_repo`는 노드의 저장소가 관문 계약을 가졌는지 본다. 없는 저장소는 판정·병합 낱말(`KindWord::gated_only`)을 그대로 쓴다.
    /// `unreported_root`는 노드가 사람이 띄운 노드이고, 사람 응답 대기가 없고, `loom done`을 부르지 않았는지 본다(`Graph::unreported_root`).
    pub fn ended(
        &self,
        node: NodeId,
        gated_repo: impl FnOnce() -> bool,
        unreported_root: impl FnOnce() -> bool,
    ) -> TurnEnd {
        let reply = self.said.lock().unwrap().remove(&node);
        let human = self.human.lock().unwrap().remove(&node);
        let called = self.called.lock().unwrap().remove(&node);
        let nudged = self.kind_nudged.lock().unwrap().remove(&node);
        let kind_line = reply
            .as_deref()
            .and_then(crate::protocol::kind_line)
            .filter(|_| !called && !nudged)
            .is_some_and(|found| !found.gated_only || gated_repo());
        let nudge = if kind_line {
            self.kind_nudged.lock().unwrap().insert(node);
            Some(KIND_NUDGE)
        } else {
            (human
                && reply.as_ref().is_some_and(|text| !text.trim().is_empty())
                && unreported_root())
            .then_some(NUDGE)
        };
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
        assert_eq!(
            turns.ended(node, || true, || true).nudge,
            None,
            "not a human turn"
        );
        turns.human(node);
        turns.said(node, "병합할까요?".into());
        assert_eq!(
            turns.ended(node, || true, || false).nudge,
            None,
            "child, asked or done"
        );
        turns.human(node);
        turns.said(node, "A안과 B안 가운데 고르시면 진행합니다.".into());
        assert_eq!(turns.ended(node, || true, || true).nudge, Some(NUDGE));
        // 다시 일한 턴은 사람 입력이 없다. 답 글은 상태 줄로 돌려준다.
        turns.said(node, "코디 결과 대기".into());
        let end = turns.ended(node, || true, || true);
        assert_eq!(
            (end.reply.as_deref(), end.nudge),
            (Some("코디 결과 대기"), None)
        );
        // 답 글이 없는 사람 턴은 막지 않는다.
        turns.human(node);
        turns.said(node, " \n".into());
        assert_eq!(turns.ended(node, || true, || true).nudge, None);
    }

    #[test]
    fn a_kind_line_without_a_loom_call_is_nudged_once() {
        let turns = TurnEnds::default();
        let node = NodeId(1);
        // 판정·병합 낱말은 관문 스크립트가 없는 저장소에서 옛 글로 쓴다.
        turns.said(node, "검사 끝.\nMERGED 출력표준".into());
        assert_eq!(
            turns.ended(node, || false, || false).nudge,
            None,
            "old repo"
        );
        turns.said(node, "MERGED 출력표준".into());
        assert_eq!(turns.ended(node, || true, || false).nudge, Some(KIND_NUDGE));
        // 다시 일한 턴이 같은 글을 써도 다시 깨우지 않는다.
        turns.said(node, "MERGED 출력표준".into());
        assert_eq!(turns.ended(node, || true, || false).nudge, None);
        // 대화 낱말과 READY는 어느 저장소에서나 본다.
        for line in [
            "QUESTION: A or B?",
            "USER DIRECTION: keep send",
            "READY 출력표준",
        ] {
            turns.said(node, format!("끝.\n{line}"));
            assert_eq!(
                turns.ended(node, || false, || false).nudge,
                Some(KIND_NUDGE),
                "{line}"
            );
            turns.said(node, "ok".into());
            turns.ended(node, || false, || false);
        }
        turns.called(node);
        turns.said(node, "STATUS: 50%".into());
        assert_eq!(
            turns.ended(node, || true, || false).nudge,
            None,
            "called loom done"
        );
        turns.said(node, "Ready for review".into());
        assert_eq!(turns.ended(node, || true, || false).nudge, None);
    }
}
