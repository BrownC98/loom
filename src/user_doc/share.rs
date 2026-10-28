use std::path::{Path, PathBuf};

use base64::Engine;
use percent_encoding::percent_decode_str;

use super::{entities, title};

const BRIDGE: &str = include_str!("bridge.js");
const BODY_END: &str = "</body>";
const REFERENCES: [(&str, char); 7] = [
    ("src=\"", '"'),
    ("src='", '\''),
    ("href=\"", '"'),
    ("href='", '\''),
    ("url(\"", '"'),
    ("url('", '\''),
    ("url(", ')'),
];
const NAME_BANS: [char; 9] = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

pub struct Shared {
    pub name: String,
    pub html: String,
}

pub fn with_bridge(html: &str) -> String {
    let script = format!("<script>{BRIDGE}</script>");
    match html.to_ascii_lowercase().rfind(BODY_END) {
        Some(at) => format!("{}{script}{}", &html[..at], &html[at..]),
        None => format!("{html}{script}"),
    }
}

pub fn shareable(
    path: &Path,
    id: &str,
    sibling: impl Fn(&str) -> Option<PathBuf>,
) -> Result<Shared, String> {
    let html = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let name = title(&html)
        .map(|t| entities(t.trim()))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| id.to_owned())
        .replace(NAME_BANS, "_");
    Ok(Shared {
        name: format!("{name}.html"),
        html: inlined(&html, sibling),
    })
}

fn inlined(html: &str, sibling: impl Fn(&str) -> Option<PathBuf>) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some((at, open, close)) = REFERENCES
        .iter()
        .filter_map(|(open, close)| rest.find(open).map(|at| (at, *open, *close)))
        .min_by_key(|(at, open, _)| (*at, std::cmp::Reverse(open.len())))
    {
        let start = at + open.len();
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find(close).unwrap_or(rest.len());
        let value = rest[..end].trim();
        match data_url(value, &sibling) {
            Some(url) => out.push_str(&url),
            None => out.push_str(&rest[..end]),
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn data_url(value: &str, sibling: impl Fn(&str) -> Option<PathBuf>) -> Option<String> {
    let file = value.split(['?', '#']).next()?;
    if file.is_empty() || file.starts_with('/') || file.contains(':') || file.ends_with(".html") {
        return None;
    }
    let path = sibling(&percent_decode_str(file).decode_utf8().ok()?)?;
    let bytes = std::fs::read(&path).ok()?;
    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    Some(format!("data:{mime};base64,{data}"))
}
