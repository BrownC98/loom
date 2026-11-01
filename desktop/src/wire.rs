use api::canvas::{CardSize, Layout};
use api::repos::RepoView;
use serde_json::{Value, json};

pub struct Node {
    pub id: u32,
    pub name: String,
    pub parent: Option<u32>,
    pub repo: u32,
    pub worktree: String,
    pub state: String,
    pub simple: bool,
}

pub struct Snapshot {
    pub nodes: Vec<Node>,
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

pub fn nodes(body: &str) -> Result<Vec<Node>, String> {
    let list: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let list = list.as_array().ok_or("GET /api/nodes is not a list")?;
    Ok(list
        .iter()
        .filter_map(|node| {
            let place = node.get("place")?;
            Some(Node {
                id: u32::try_from(node.get("id")?.as_u64()?).ok()?,
                name: node.get("name")?.as_str()?.to_owned(),
                parent: node
                    .get("parent")
                    .and_then(Value::as_u64)
                    .and_then(|id| u32::try_from(id).ok()),
                repo: u32::try_from(place.get("repo")?.as_u64()?).ok()?,
                worktree: place.get("worktree")?.as_str()?.to_owned(),
                state: node.get("state")?.get("kind")?.as_str()?.to_owned(),
                simple: node.get("cardView").and_then(Value::as_str) == Some("simple"),
            })
        })
        .collect())
}

pub fn snapshot(
    nodes_body: &str,
    repos: &str,
    spots: &str,
    sizes: &str,
) -> Result<Snapshot, String> {
    let parse_error = |what: &str, e: serde_json::Error| format!("{what}: {e}");
    Ok(Snapshot {
        nodes: nodes(nodes_body)?,
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
