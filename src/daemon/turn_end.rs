//! 턴 끝 두 가지.
//! 질문 검사: 사람이 띄운 노드가 글로만 묻고 `loom ask` 없이 턴을 끝내면 한 번 다시 일하게 한다.
//! 받은편지함에는 아무것도 걸지 않는다. 에이전트가 사람에게 묻는 것인지 스스로 정한다.
//! 근거: docs/design.md 「사람 응답 대기」, 2026-10-21 사람 승인 "부모 없는 노드가 ?로 끝나는 문장을 남기고 loom ask 없이 턴을 끝내면 한 번 막고 다시 일하게 한다".
//! 완료 알림: 사람이 연 턴이 끝나면 받은편지함에 완료를 건다(`Graph::finished`). 에이전트끼리의 메시지로 연 턴은 걸지 않는다.
//! 근거: docs/design.md 「완료 알림」, 2026-10-22 목업 M37 v2 승인.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use super::App;
use crate::graph::NodeId;

/// 다시 일하게 할 때 에이전트에 넘기는 글. 질문이 아니면 `.` 한 글자로 끝내게 해 토큰을 아낀다(2026-10-22 사람 지시 "질문아닐때의 훅 응답은 매우 짧은 텍스트로 끝내기").
const NUDGE: &str = "Your text ends with '?' but you did not call loom ask. A question to the human: call loom ask now, then end the turn. Not a question: reply only \".\" and end the turn.";

/// loom이 에이전트에 넣는 메시지의 첫머리(`profile::message_text`). Cursor는 stop 훅의 `followup_message`도 사람 입력 훅으로 받는다.
const MESSAGE_HEAD: &str = "loom message #";

/// 턴 끝에 할 일.
#[derive(PartialEq, Debug)]
pub enum Ended {
    /// 다시 일하게 한다(질문 검사).
    Nudge(&'static str),
    /// 사람이 연 턴이 끝났다. 받은편지함에 완료를 건다.
    Done {
        instruction: String,
        result: String,
    },
    Quiet,
}

/// 사람이 연 턴.
struct HumanTurn {
    /// 그 턴을 연 사람의 지시.
    instruction: String,
    /// 다시 일하게 하기 전 턴의 답 글. 다시 일한 턴이 「.」만 답하면(질문이 아니었다) 이 글이 결과다.
    before_nudge: Option<String>,
}

#[derive(Default)]
pub struct TurnEnds {
    /// 노드마다 이번 턴의 마지막 답 글. 턴 끝에 꺼낸다.
    said: Mutex<HashMap<NodeId, String>>,
    /// 바로 앞 턴 끝에 다시 일하게 한 노드. 다음 턴 끝은 검사하지 않는다(한 번만 막는다).
    nudged: Mutex<HashSet<NodeId>>,
    /// 사람이 연 턴이 도는 노드. 턴 끝에 꺼낸다.
    human: Mutex<HashMap<NodeId, HumanTurn>>,
}

impl TurnEnds {
    /// 사람이 입력해 턴이 시작했다. 같은 턴의 뒤 입력은 버린다(일을 맡긴 지시는 첫 입력이다).
    /// loom이 넣은 글(메시지, 다시 일하게 한 글)은 사람의 입력이 아니다.
    pub fn human(&self, node: NodeId, instruction: &str) {
        if instruction.starts_with(MESSAGE_HEAD) || instruction == NUDGE {
            return;
        }
        self.human
            .lock()
            .unwrap()
            .entry(node)
            .or_insert_with(|| HumanTurn {
                instruction: instruction.to_owned(),
                before_nudge: None,
            });
    }

    /// 지금 턴을 사람이 열었다. Codex는 이때만 답 글을 읽는다.
    pub fn human_turn(&self, node: NodeId) -> bool {
        self.human.lock().unwrap().contains_key(&node)
    }

    /// 에이전트가 답 글을 냈다. 같은 턴의 앞 글은 버린다.
    pub fn said(&self, node: NodeId, text: String) {
        self.said.lock().unwrap().insert(node, text);
    }

    /// 턴이 끝났다. 할 일을 돌려준다.
    /// `unasked_root`는 노드가 사람이 띄운 노드이고 사람 응답 대기가 없는지 본다(`Graph::unasked_root`).
    /// 이번 턴에 `loom ask`로 물었으면 대기가 남아 있다. 다시 일하게 한 턴은 사람이 연 턴을 잇는다. 완료는 그 턴 끝에 건다.
    pub fn ended(&self, node: NodeId, unasked_root: impl FnOnce() -> bool) -> Ended {
        let said = self.said.lock().unwrap().remove(&node);
        let nudged = self.nudged.lock().unwrap().remove(&node);
        if !nudged && said.as_deref().is_some_and(asks_in_text) && unasked_root() {
            self.nudged.lock().unwrap().insert(node);
            if let Some(turn) = self.human.lock().unwrap().get_mut(&node) {
                turn.before_nudge = said;
            }
            return Ended::Nudge(NUDGE);
        }
        match self.human.lock().unwrap().remove(&node) {
            Some(turn) => Ended::Done {
                instruction: turn.instruction,
                result: match said {
                    Some(s) if s.trim() != "." => s,
                    _ => turn.before_nudge.unwrap_or_default(),
                },
            },
            None => Ended::Quiet,
        }
    }
}

impl App {
    /// 턴이 끝났다. 다시 일하게 할 글이 있으면 돌려준다. 사람이 연 턴이면 받은편지함에 완료를 건다.
    pub(super) fn turn_end(&self, node: NodeId) -> Option<&'static str> {
        let ended = self.turn_ends.ended(node, || {
            self.graph.unasked_root(node).unwrap_or_else(|e| {
                eprintln!("loom: 노드 {}의 턴 끝을 검사하지 못했다: {e}", node.0);
                false
            })
        });
        match ended {
            Ended::Nudge(nudge) => Some(nudge),
            Ended::Done {
                instruction,
                result,
            } => {
                if let Err(e) = self.graph.finished(node, &instruction, &result) {
                    eprintln!("loom: 노드 {}의 완료 알림을 걸지 못했다: {e}", node.0);
                }
                None
            }
            Ended::Quiet => None,
        }
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
        assert_eq!(turns.ended(node, || false), Ended::Quiet, "child or asked");
        turns.said(node, "병합할까요?".into());
        assert_eq!(turns.ended(node, || true), Ended::Nudge(NUDGE));
        // 다시 일한 턴이 또 묻고 끝나도 막지 않는다.
        turns.said(node, "병합할까요?".into());
        assert_eq!(turns.ended(node, || true), Ended::Quiet);
        turns.said(node, "병합할까요?".into());
        assert_eq!(turns.ended(node, || true), Ended::Nudge(NUDGE));
        // 답 글이 없는 턴 끝은 막지 않는다. 그래도 한 번 막은 표시는 지운다.
        assert_eq!(turns.ended(node, || true), Ended::Quiet);
        turns.said(node, "끝냈다.".into());
        assert_eq!(turns.ended(node, || true), Ended::Quiet);
        turns.said(node, "병합할까요?".into());
        assert_eq!(turns.ended(node, || true), Ended::Nudge(NUDGE));
    }

    #[test]
    fn only_a_human_turn_ends_done() {
        let turns = TurnEnds::default();
        let node = NodeId(1);
        // 에이전트 메시지로 연 턴(Cursor followup 포함)은 완료가 아니다.
        turns.human(node, "loom message #3 (from: lead, mode: queue)\nDo it.");
        turns.said(node, "끝냈다.".into());
        assert_eq!(turns.ended(node, || true), Ended::Quiet);
        // 사람이 연 턴: 첫 입력이 지시다.
        turns.human(node, "버튼 고쳐");
        turns.human(node, "그리고 시험도");
        assert!(turns.human_turn(node));
        turns.said(node, "고쳤다.".into());
        assert_eq!(
            turns.ended(node, || true),
            Ended::Done {
                instruction: "버튼 고쳐".into(),
                result: "고쳤다.".into()
            }
        );
        assert!(!turns.human_turn(node));
        // 글로 물어 다시 일하게 한 턴은 사람이 연 턴을 잇는다. 그 턴 끝에 완료를 건다. 「.」은 결과가 아니다.
        turns.human(node, "병합해");
        turns.said(node, "병합할까요?".into());
        assert_eq!(turns.ended(node, || true), Ended::Nudge(NUDGE));
        turns.human(node, NUDGE);
        turns.said(node, ".".into());
        assert_eq!(
            turns.ended(node, || true),
            Ended::Done {
                instruction: "병합해".into(),
                result: "병합할까요?".into()
            }
        );
    }
}
