use serde_json::{Value, json};

pub struct Node {
    pub id: u32,
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
            Some(Node {
                id: u32::try_from(node.get("id")?.as_u64()?).ok()?,
            })
        })
        .collect())
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
