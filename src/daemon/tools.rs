use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

use super::App;
use api::ids::NodeId;

const SHOWN_EVERY: Duration = Duration::from_secs(1);

#[derive(Default)]
pub struct Tools {
    last: Mutex<HashMap<NodeId, String>>,
    changed: Notify,
}

impl Tools {
    pub fn used(&self, node: NodeId, tool: String) {
        let before = self.last.lock().unwrap().insert(node, tool.clone());
        if before.as_ref() != Some(&tool) {
            self.changed.notify_one();
        }
    }

    pub fn ended(&self, node: NodeId) {
        self.last.lock().unwrap().remove(&node);
    }

    pub fn of(&self, node: NodeId) -> Option<String> {
        self.last.lock().unwrap().get(&node).cloned()
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
