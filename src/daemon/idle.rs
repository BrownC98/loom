//! 쉬는 Claude 노드의 대기. Claude 비동기 훅(`asyncRewake`)이 턴이 끝날 때 `loom hook`으로 연다.
//! 노드마다 하나만 산다. Claude는 새 턴이 시작해도 옛 비동기 훅을 끝내지 않기 때문이다(실측: 2.1.305).
//! 근거: docs/design.md 「전달 지점」.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use crate::graph::NodeId;

#[derive(Default)]
pub struct IdleWaits {
    /// 노드마다 지금 대기를 끝내는 신호.
    ends: Mutex<HashMap<NodeId, Arc<Notify>>>,
}

impl IdleWaits {
    /// 새 대기를 연다. 같은 노드의 옛 대기는 끝난다. 돌려준 신호가 오면 이 대기를 끝낸다.
    pub fn open(&self, node: NodeId) -> Arc<Notify> {
        let end = Arc::new(Notify::new());
        if let Some(old) = self.ends.lock().unwrap().insert(node, end.clone()) {
            old.notify_one();
        }
        end
    }

    /// 노드가 턴을 시작했거나 프로세스가 끝났다. 그 노드의 대기를 끝낸다.
    pub fn end(&self, node: NodeId) {
        if let Some(old) = self.ends.lock().unwrap().remove(&node) {
            old.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
