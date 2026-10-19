//! 노드 목록과 상태. 바뀔 때마다 화면에 노드 하나의 이벤트를 보낸다.
//! 지금은 메모리에 둔다. 2단위에서 SQLite(`~/.loom/loom.db`)로 옮긴다.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::Serialize;
use tokio::sync::broadcast;
use ts_rs::TS;

use crate::profile::Profile;

/// 화면 하나가 밀려도 버티는 이벤트 수.
const EVENT_QUEUE: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, TS)]
#[ts(export)]
pub struct NodeId(pub u32);

#[derive(Clone, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum NodeState {
    Running,
    Exited { code: u32 },
}

#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct NodeView {
    pub id: NodeId,
    pub name: String,
    pub profile: Profile,
    pub state: NodeState,
}

pub struct Registry {
    inner: Mutex<Inner>,
    events: broadcast::Sender<NodeView>,
}

#[derive(Default)]
struct Inner {
    last_id: u32,
    nodes: BTreeMap<NodeId, NodeView>,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            inner: Mutex::default(),
            events: broadcast::channel(EVENT_QUEUE).0,
        }
    }

    /// 노드를 띄우기 전에 id를 받는다. 띄우지 못하면 이 id는 버린다.
    pub fn reserve_id(&self) -> NodeId {
        let mut inner = self.inner.lock().unwrap();
        inner.last_id += 1;
        NodeId(inner.last_id)
    }

    pub fn insert(&self, view: NodeView) {
        let mut inner = self.inner.lock().unwrap();
        inner.nodes.insert(view.id, view.clone());
        let _ = self.events.send(view);
    }

    pub fn set_state(&self, id: NodeId, state: NodeState) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(view) = inner.nodes.get_mut(&id) {
            view.state = state;
            let _ = self.events.send(view.clone());
        }
    }

    /// 지금 목록과 그 뒤의 이벤트를 빈틈없이 받는다.
    pub fn subscribe(&self) -> (Vec<NodeView>, broadcast::Receiver<NodeView>) {
        let inner = self.inner.lock().unwrap();
        (
            inner.nodes.values().cloned().collect(),
            self.events.subscribe(),
        )
    }
}
