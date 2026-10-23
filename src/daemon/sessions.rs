//! 노드마다 지금 프로세스의 세션 단계. 화면 감지가 본다(`screen::Session`).
//! 옛 프로세스의 늦은 호출이 다시 띄운 프로세스의 세션을 시작하지 못하게 프로세스의 토큰으로 가린다.
//! 근거: docs/design.md 「사람 응답 대기」.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use tokio::sync::watch;

use crate::graph::NodeId;
use crate::screen::Session;

#[derive(Default)]
pub struct Sessions {
    /// 노드마다 지금 프로세스의 토큰과 세션 단계.
    nodes: Mutex<HashMap<NodeId, (String, watch::Sender<Session>)>>,
}

impl Sessions {
    /// `token`을 받은 새 프로세스의 세션을 시작 전으로 둔다. 같은 노드의 옛 세션은 끝난다(받는 쪽의 `changed`가 실패한다).
    pub fn launched(&self, node: NodeId, token: &str) -> watch::Receiver<Session> {
        let (session, started) = watch::channel(Session::NotStarted);
        self.nodes
            .lock()
            .unwrap()
            .insert(node, (token.to_owned(), session));
        started
    }

    /// `token`을 낸 프로세스의 런타임이 세션을 시작했다. 지금 프로세스의 토큰이 아니면 무시한다.
    /// 계정 전환은 새 프로세스를 띄운 뒤에 새 토큰을 그래프에 쓴다. 그 사이에 온 옛 토큰의 호출도 여기서 걸린다.
    pub fn started(&self, node: NodeId, token: &str) {
        if let Some((_, session)) = self
            .nodes
            .lock()
            .unwrap()
            .get(&node)
            .filter(|(current, _)| current == token)
        {
            session
                .send_if_modified(|s| std::mem::replace(s, Session::Started) != Session::Started);
        }
    }

    /// 노드 프로세스가 끝났다.
    pub fn exited(&self, node: NodeId) {
        self.nodes.lock().unwrap().remove(&node);
    }
}

/// 노드마다 loom이 첫 입력(지시문, 다시 띄울 때의 글)을 주고 띄웠는지, 깨우는 입력을 쳤는지. 그 뒤 첫 사람 입력 훅은 그 입력이다:
/// Claude는 시작 인자의 첫 입력을 바로 제출하고 `UserPromptSubmit`을 부른다. 그 턴은 부모에게 `human-input`으로 알리지 않는다.
/// 글을 비교하지 않는다: Claude가 넘기는 글이 시작 인자와 같다는 보장이 없다.
#[derive(Default)]
pub struct FirstInputs(Mutex<HashSet<NodeId>>);

impl FirstInputs {
    pub fn launched(&self, node: NodeId, first_input: bool) {
        let mut nodes = self.0.lock().unwrap();
        match first_input {
            true => nodes.insert(node),
            false => nodes.remove(&node),
        };
    }

    /// loom이 쉬는 노드에 입력을 친다(`wake`). 다음 사람 입력 훅은 그 입력이다.
    pub fn typing(&self, node: NodeId) {
        self.0.lock().unwrap().insert(node);
    }

    /// 이 입력이 loom이 준 첫 입력인가. 띄운 뒤 처음 한 번만 참이다.
    pub fn injected(&self, node: NodeId) -> bool {
        self.0.lock().unwrap().remove(&node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 다시 띄운 노드에서 옛 프로세스의 훅·`loom` 명령이 늦게 와도 새 프로세스의 첫 창(폴더 신뢰, 로그인)을 놓치지 않는다.
    #[test]
    fn a_late_call_of_the_old_process_does_not_start_the_new_session() {
        let sessions = Sessions::default();
        let old = sessions.launched(NodeId(1), "old");
        let new = sessions.launched(NodeId(1), "new");
        assert!(old.has_changed().is_err(), "옛 세션은 끝난다");
        sessions.started(NodeId(1), "old");
        assert!(*new.borrow() == Session::NotStarted);
        sessions.started(NodeId(1), "new");
        assert!(*new.borrow() == Session::Started);
    }

    #[test]
    fn only_the_first_prompt_after_a_launch_with_a_brief_is_injected() {
        let inputs = FirstInputs::default();
        inputs.launched(NodeId(1), true);
        inputs.launched(NodeId(2), false);
        assert!(inputs.injected(NodeId(1)));
        assert!(!inputs.injected(NodeId(1)), "the human typed this one");
        assert!(!inputs.injected(NodeId(2)));
    }
}
