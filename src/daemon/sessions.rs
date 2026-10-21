//! 노드마다 지금 프로세스의 세션 단계. 화면 감지가 본다(`screen::Session`).
//! 옛 프로세스의 늦은 호출이 다시 띄운 프로세스의 세션을 시작하지 못하게 프로세스의 토큰으로 가린다.
//! 근거: docs/design.md 「사람 응답 대기」.

use std::collections::HashMap;
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
}
