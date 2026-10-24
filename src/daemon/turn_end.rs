use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{App, TurnId};
use crate::graph::{Entry, NodeId, TurnCheck};

pub(super) const IDLE_GRACE: Duration = Duration::from_secs(60);

const NUDGE: &str = "You ended the turn with text but without loom ask or loom done. If the text asks the human anything (a question, a choice, an approval), call loom ask \"<one line>\", then end your turn. If you finished a result the human can use, call loom done \"<what you did>\" --asked \"<the human's instruction>\". Otherwise reply with only a few words in the language the human uses with you that brief your current work or state. Do not name this check. Example in Korean: \"코디 결과 대기\". Then end your turn.";

const KIND_NUDGE: &str = "Your last reply has a line that starts with a kind word (QUESTION, ANSWER, STATUS, DONE, READY, MERGED, USER DIRECTION, ...), but you called no loom verb in this turn. Text in the terminal reaches nobody. If you meant a message, send it now with its verb: loom ask, loom answer, loom done, loom note, or a gate verb (see loom --help). Otherwise reply with a few words without that line. Then end your turn.";

#[derive(Default)]
pub struct TurnEnds {
    said: Mutex<HashMap<NodeId, String>>,
    human: Mutex<HashSet<NodeId>>,
    called: Mutex<HashSet<NodeId>>,
    kind_nudged: Mutex<HashSet<NodeId>>,
}

impl TurnEnds {
    pub fn said(&self, node: NodeId, text: String) {
        self.said.lock().unwrap().insert(node, text);
    }

    pub fn human(&self, node: NodeId) {
        self.human.lock().unwrap().insert(node);
    }

    pub fn called(&self, node: NodeId) {
        self.called.lock().unwrap().insert(node);
    }

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
            self.kind_nudged.lock().unwrap().insert(node);
            Some(TurnCheck::KindLine)
        } else {
            (human
                && reply.as_ref().is_some_and(|text| !text.trim().is_empty())
                && unreported_root())
            .then_some(TurnCheck::Unreported)
        };
        TurnEnd { reply, nudge }
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
    }
}

impl App {
    pub(super) fn said(self: &Arc<Self>, node: NodeId, text: String, turn: Option<TurnId>) {
        self.turn_ends.said(node, text);
        let Some(said) = turn.and_then(|turn| self.idle.said(node, turn)) else {
            return;
        };
        let app = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(IDLE_GRACE).await;
            if !app.idle.missed(node, said) {
                return;
            }
            let active = match app.graph.active() {
                Ok(nodes) => nodes.into_iter().find(|n| n.id == node),
                Err(e) => {
                    eprintln!("loom: 노드 {}를 찾지 못했다: {e}", node.0);
                    return;
                }
            };
            let Some(active) = active else {
                return;
            };
            if let Err(e) = app
                .graph
                .record_about(node, |node| Entry::NoIdleWait { node })
            {
                eprintln!(
                    "loom: 노드 {}의 빠진 쉬는 대기를 남기지 못했다: {e}",
                    node.0
                );
            }
            if let Err(e) = app.relaunch_or_queue(active).await {
                eprintln!(
                    "loom: 쉬는 대기가 열리지 않은 노드 {}를 다시 띄우지 못했다: {e}",
                    node.0
                );
            }
        });
    }

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
        let check = nudge.filter(|_| !self.relaunches.holds_mail(node))?;
        if let Err(e) = self
            .graph
            .record_about(node, |node| Entry::Nudged { node, check })
        {
            eprintln!("loom: 노드 {}의 턴 끝 검사를 남기지 못했다: {e}", node.0);
        }
        Some(nudge_text(check))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_human_turn_of_an_unreported_root_is_nudged_once() {
        let turns = TurnEnds::default();
        let node = NodeId(1);
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
        assert_eq!(
            turns.ended(node, || true, || true).nudge,
            Some(TurnCheck::Unreported)
        );
        turns.said(node, "코디 결과 대기".into());
        let end = turns.ended(node, || true, || true);
        assert_eq!(
            (end.reply.as_deref(), end.nudge),
            (Some("코디 결과 대기"), None)
        );
        turns.human(node);
        turns.said(node, " \n".into());
        assert_eq!(turns.ended(node, || true, || true).nudge, None);
    }

    #[test]
    fn a_kind_line_without_a_loom_call_is_nudged_once() {
        let turns = TurnEnds::default();
        let node = NodeId(1);
        turns.said(node, "검사 끝.\nMERGED 출력표준".into());
        assert_eq!(
            turns.ended(node, || false, || false).nudge,
            None,
            "old repo"
        );
        turns.said(node, "MERGED 출력표준 abc\nQUESTION: 지울까?".into());
        assert_eq!(
            turns.ended(node, || false, || false).nudge,
            Some(TurnCheck::KindLine),
            "old repo, later question"
        );
        turns.said(node, "ok".into());
        turns.ended(node, || false, || false);
        turns.said(node, "MERGED 출력표준".into());
        assert_eq!(
            turns.ended(node, || true, || false).nudge,
            Some(TurnCheck::KindLine)
        );
        turns.said(node, "MERGED 출력표준".into());
        assert_eq!(turns.ended(node, || true, || false).nudge, None);
        for line in [
            "QUESTION: A or B?",
            "USER DIRECTION: keep send",
            "READY 출력표준",
        ] {
            turns.said(node, format!("끝.\n{line}"));
            assert_eq!(
                turns.ended(node, || false, || false).nudge,
                Some(TurnCheck::KindLine),
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
