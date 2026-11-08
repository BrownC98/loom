use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

use super::App;
use api::ids::NodeId;

const SHOWN_EVERY: Duration = Duration::from_secs(1);

#[derive(Default)]
pub struct Tools {
    last: Mutex<HashMap<NodeId, (u64, String)>>,
    next: AtomicU64,
    changed: Notify,
}

impl Tools {
    pub fn used(&self, node: NodeId, tool: String) {
        let mark = self.next.fetch_add(1, Ordering::Relaxed);
        let before = self.last.lock().unwrap().insert(node, (mark, tool.clone()));
        if before.map(|(_, t)| t).as_ref() != Some(&tool) {
            self.changed.notify_one();
        }
    }

    pub fn mark(&self, node: NodeId) -> Option<u64> {
        self.last.lock().unwrap().get(&node).map(|(mark, _)| *mark)
    }

    pub fn ended(&self, node: NodeId, mark: Option<u64>) {
        let mut last = self.last.lock().unwrap();
        if mark.is_some() && last.get(&node).map(|(m, _)| *m) == mark {
            last.remove(&node);
        }
    }

    pub fn of(&self, node: NodeId) -> Option<String> {
        self.last.lock().unwrap().get(&node).map(|(_, t)| t.clone())
    }
}

impl App {
    pub(super) fn show_tools(self: &Arc<Self>) {
        let app = self.clone();
        tokio::spawn(async move {
            loop {
                app.tools.changed.notified().await;
                app.graph.publish_nodes();
                tokio::time::sleep(SHOWN_EVERY).await;
            }
        });
    }
}
