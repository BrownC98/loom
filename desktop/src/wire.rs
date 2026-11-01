use api::canvas::{CardSize, Layout};
use api::graph::NodeView;
use api::repos::RepoView;
use serde_json::{Value, json};

pub struct Snapshot {
    pub nodes: Vec<NodeView>,
    pub repos: Vec<RepoView>,
    pub layout: Layout,
    pub sizes: Vec<CardSize>,
}

pub enum Notice {
    Gone,
    Size { cols: u16, rows: u16 },
    Settled,
    Beat,
}

pub fn snapshot(
    nodes_body: &str,
    repos: &str,
    spots: &str,
    sizes: &str,
) -> Result<Snapshot, String> {
    let parse_error = |what: &str, e: serde_json::Error| format!("{what}: {e}");
    Ok(Snapshot {
        nodes: serde_json::from_str(nodes_body).map_err(|e| parse_error("GET /api/nodes", e))?,
        repos: serde_json::from_str(repos).map_err(|e| parse_error("GET /api/repos", e))?,
        layout: serde_json::from_str(spots).map_err(|e| parse_error("GET /api/spots", e))?,
        sizes: serde_json::from_str(sizes).map_err(|e| parse_error("GET /api/sizes", e))?,
    })
}

pub fn reloads(text: &str) -> bool {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|event| event.get("kind")?.as_str().map(str::to_owned))
        .is_some_and(|kind| kind == "nodes" || kind == "viewChanged")
}

pub fn notice(text: &str) -> Option<Notice> {
    let value: Value = serde_json::from_str(text).ok()?;
    match value.as_str() {
        Some("gone") => return Some(Notice::Gone),
        Some("settled") => return Some(Notice::Settled),
        _ => {}
    }
    if value.get("beat").is_some() {
        return Some(Notice::Beat);
    }
    let size = value.get("size")?;
    Some(Notice::Size {
        cols: u16::try_from(size.get("cols")?.as_u64()?).ok()?,
        rows: u16::try_from(size.get("rows")?.as_u64()?).ok()?,
    })
}

pub fn resize(cols: u16, rows: u16) -> String {
    json!({ "resize": { "cols": cols, "rows": rows } }).to_string()
}

pub fn pace_front() -> String {
    json!({ "pace": "front" }).to_string()
}
