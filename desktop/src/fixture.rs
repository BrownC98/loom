//! Test data: a store with two nodes and no daemon.

use serde_json::json;

use crate::link::Snapshot;

pub const BASE: &str = "http://127.0.0.1:9";

fn node(id: u32, name: &str, view: Option<&str>) -> serde_json::Value {
    json!({
        "id": id, "name": name, "agent": null, "parent": null,
        "state": {"kind": "running"}, "process": {"kind": "idle"},
        "unread": 0, "model": null, "effort": null,
        "place": {"repo": 1, "worktree": "/tmp/loom-test"},
        "bookmark": null, "cardView": view, "reply": null, "waits": [], "relaunch": null
    })
}

pub fn snapshot(view: Option<&str>) -> Snapshot {
    fn value<T: serde::de::DeserializeOwned>(v: serde_json::Value) -> T {
        serde_json::from_value(v).expect("fixture")
    }
    Snapshot {
        nodes: value(json!([node(1, "alpha", view), node(2, "beta", view)])),
        repos: value(json!([{
            "id": 1, "name": "r", "path": "/tmp/loom-test", "worktrees": [],
            "branches": [], "hasCommits": true, "place": {"kind": "pc"}, "shownPath": "/tmp/loom-test"
        }])),
        layout: value(json!({"placed": [], "arranged": []})),
        sizes: Vec::new(),
    }
}
