//! 쉬는 Claude 노드의 대기. Claude 비동기 훅(`asyncRewake`)이 턴이 끝날 때 `loom hook`으로 연다.
//! 노드마다 하나만 산다. Claude는 새 턴이 시작해도 옛 비동기 훅을 끝내지 않기 때문이다(실측: 2.1.305).
//! 근거: docs/design.md 「전달 지점」.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use crate::graph::NodeId;

#[derive(Default)]
pub struct IdleWaits {
    /// 노드마다 지금 대기를 끝내는 신호.
    ends: Mutex<HashMap<NodeId, Arc<Notify>>>,
    /// 턴 안의 훅이 불린 뒤 턴 끝 훅이 아직 오지 않은 노드. 데몬이 다시 시작하면 비어 있다.
    /// 쉬는 대기가 없다고 턴 중으로 보지 않는다. 옛 실행 설정의 노드는 턴이 끝나도 대기를 열지 않는다(동기 `Stop` 훅).
    turns: Mutex<HashSet<NodeId>>,
}

impl IdleWaits {
    /// 새 대기를 연다. 같은 노드의 옛 대기는 끝난다. 돌려준 신호가 오면 이 대기를 끝낸다.
    pub fn open(&self, node: NodeId) -> Arc<Notify> {
        self.turns.lock().unwrap().remove(&node);
        let end = Arc::new(Notify::new());
        if let Some(old) = self.ends.lock().unwrap().insert(node, end.clone()) {
            old.notify_one();
        }
        end
    }

    /// 노드가 턴 중이다. 다시 띄우기가 이 턴의 끝을 기다린다.
    pub fn in_turn(&self, node: NodeId) -> bool {
        self.turns.lock().unwrap().contains(&node)
    }

    /// 턴 안의 훅이 불렸다. 그 노드의 대기를 끝낸다.
    pub fn turn_started(&self, node: NodeId) {
        self.end(node);
        self.turns.lock().unwrap().insert(node);
    }

    /// 프로세스가 끝났거나 다시 띄운다. 그 노드의 대기를 끝낸다.
    pub fn end(&self, node: NodeId) {
        self.turns.lock().unwrap().remove(&node);
        if let Some(old) = self.ends.lock().unwrap().remove(&node) {
            old.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 대기가 없어도 턴 훅이 오기 전에는 턴 중이 아니다. 다시 시작한 데몬은 쉬는 노드를 바로 다시 띄운다.
    #[test]
    fn only_a_turn_hook_until_the_turn_end_counts_as_a_turn() {
        let waits = IdleWaits::default();
        assert!(!waits.in_turn(NodeId(1)));
        waits.turn_started(NodeId(1));
        assert!(waits.in_turn(NodeId(1)));
        let _ = waits.open(NodeId(1));
        assert!(!waits.in_turn(NodeId(1)));
        waits.turn_started(NodeId(1));
        waits.end(NodeId(1));
        assert!(!waits.in_turn(NodeId(1)));
    }

    #[tokio::test]
    async fn a_newer_wait_or_a_turn_ends_the_old_wait() {
        let waits = IdleWaits::default();
        let first = waits.open(NodeId(1));
        let second = waits.open(NodeId(1));
        // `notify_one`은 신호를 남긴다. 아직 기다리지 않은 대기도 끝난다.
        first.notified().await;
        let other = waits.open(NodeId(2));
        waits.end(NodeId(1));
        second.notified().await;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), other.notified())
                .await
                .is_err()
        );
    }
}
