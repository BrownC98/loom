use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::task;

/// 화면이 그리는 글 조각. 화면은 링크를 찾지 않는다.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "part", rename_all = "camelCase")]
#[ts(export)]
pub enum TextPart {
    Text {
        text: String,
    },
    /// `file://`·`http(s)://` 주소.
    Link {
        label: String,
        href: String,
    },
    /// 이 글에 붙은 문서(`DOC_LINK`).
    Doc {
        label: String,
    },
}

pub fn parts(text: &str) -> Vec<TextPart> {
    let mut parts = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let (before, link) = rest.split_at(open);
        plain.push_str(before);
        match parse_link(link) {
            Some((label, target, len)) if !link[..len - 1].ends_with('>') && drawn(target) => {
                if !plain.is_empty() {
                    parts.push(TextPart::Text {
                        text: std::mem::take(&mut plain),
                    });
                }
                let label = label.to_owned();
                parts.push(if target == DOC_LINK {
                    TextPart::Doc { label }
                } else {
                    TextPart::Link {
                        label,
                        href: target.to_owned(),
                    }
                });
                rest = &link[len..];
            }
            _ => {
                plain.push('[');
                rest = &link[1..];
            }
        }
    }
    plain.push_str(rest);
    if !plain.is_empty() {
        parts.push(TextPart::Text { text: plain });
    }
    parts
}

const PATH_RESERVED: &AsciiSet = &CONTROLS.add(b'%').add(b'#').add(b'?');

pub fn path_link(path: &str) -> TextPart {
    let slashed = path.replace('\\', "/");
    let marker = format!("/{}/", task::ROOT.join("/"));
    let label = slashed
        .split_once(&marker)
        .and_then(|(_, task)| task.split_once('/'))
        .map_or(path, |(_, inside)| inside);
    TextPart::Link {
        label: label.to_owned(),
        href: if is_url(path) {
            drawable_form(path)
        } else {
            drawable_form(&utf8_percent_encode(path, PATH_RESERVED).to_string())
        },
    }
}

pub fn parse_link(link: &str) -> Option<(&str, &str, usize)> {
    let (label, after) = link[1..].split_once("](")?;
    let (target, end) = match after.strip_prefix('<') {
        Some(wrapped) => {
            let (target, tail) = wrapped.split_once('>')?;
            tail.starts_with(')')
                .then_some((target, target.len() + 3))?
        }
        None => {
            let target = after.split_once(')')?.0;
            if target.contains(char::is_whitespace) {
                return None;
            }
            (target, target.len() + 1)
        }
    };
    let ok = !label.is_empty()
        && !label.contains(['[', ']', '\n'])
        && !target.is_empty()
        && !target.contains('\n');
    ok.then_some((label, target, label.len() + 3 + end))
}

pub const DOC_LINK: &str = "loom:doc";

pub fn bare_link(text: &str) -> Option<String> {
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let (before, link) = rest.split_at(open);
        if let Some(found) = bare_in(before) {
            return Some(found);
        }
        rest = match parse_link(link) {
            Some((_, target, len)) if link[..len - 1].ends_with('>') => {
                return Some(target.to_owned());
            }
            Some((label, target, _)) if !drawn(target) => {
                let from = label.len() + 3;
                return Some(balanced_target(&link[from..]).to_owned());
            }
            Some((_, _, len)) => &link[len..],
            None => {
                if let Some(target) = spaced_target(link) {
                    return Some(target.to_owned());
                }
                &link[1..]
            }
        };
    }
    bare_in(rest)
}

fn spaced_target(link: &str) -> Option<&str> {
    let (label, after) = link[1..].split_once("](")?;
    let target = after.split_once(')')?.0.trim();
    let ok = !label.is_empty()
        && !label.contains(['[', ']', '\n'])
        && !target.contains(['\n', '<'])
        && target.contains(' ')
        && locates(target);
    ok.then_some(target)
}

fn locates(target: &str) -> bool {
    let drive = matches!(
        target.as_bytes(),
        [letter, b':', b'\\' | b'/', ..] if letter.is_ascii_alphabetic()
    );
    is_url(target) || target.starts_with(['/', '\\', '~']) || drive
}

fn drawn(target: &str) -> bool {
    target == DOC_LINK
        || (!target.contains(['(', '<', '>'])
            && ["file://", "http://", "https://"]
                .iter()
                .any(|s| target.strip_prefix(s).is_some_and(|r| !r.is_empty())))
}

pub fn drawable_form(found: &str) -> String {
    let url = if is_url(found) {
        found.to_owned()
    } else {
        let path = found.replace('\\', "/");
        match path.as_bytes() {
            [b'/', b'/', ..] => format!("file:{path}"),
            [_, b':', ..] => format!("file:///{path}"),
            _ => format!("file://{path}"),
        }
    };
    url.replace(' ', "%20")
        .replace('(', "%28")
        .replace(')', "%29")
        .replace('<', "%3C")
        .replace('>', "%3E")
}

fn is_url(text: &str) -> bool {
    ["file://", "http://", "https://"]
        .iter()
        .any(|s| text.starts_with(s))
}

fn starts_token(before: Option<char>) -> bool {
    before.is_none_or(|c| !c.is_alphanumeric() && !"/\\.-_~".contains(c))
}

fn bare_in(segment: &str) -> Option<String> {
    let chars: Vec<(usize, char)> = segment.char_indices().collect();
    (0..chars.len()).find_map(|i| {
        let (at, c) = chars[i];
        let tail = &segment[at..];
        let before = i.checked_sub(1).map(|j| chars[j].1);
        let url = ["http://", "https://", "file://"]
            .iter()
            .any(|s| tail.starts_with(s));
        let unix = c == '/' && {
            let token = tail.split(char::is_whitespace).next().unwrap_or(tail);
            token[1..].find('/').is_some_and(|n| n > 0)
        };
        let drive = c.is_ascii_alphabetic()
            && tail[1..].starts_with(':')
            && (tail[2..].starts_with('\\') || tail[2..].starts_with('/'));
        let unc = tail.starts_with("\\\\") && chars.get(i + 2).is_some();
        (url || (starts_token(before) && (unix || drive || unc))).then(|| span(tail))
    })
}

fn span(tail: &str) -> String {
    let cut = tail.find(['\n', '`', '"', '\'', '<', '>']);
    let quoted = cut.is_some_and(|p| matches!(tail.as_bytes()[p], b'`' | b'"' | b'\''));
    let found = &tail[..cut.unwrap_or(tail.len())];
    if quoted {
        return found.to_owned();
    }
    trim_tail(found.split(char::is_whitespace).next().unwrap_or(found)).to_owned()
}

fn balanced_target(text: &str) -> &str {
    let mut depth = 0usize;
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => return &text[..i],
            ')' => depth -= 1,
            c if c.is_whitespace() => return &text[..i],
            _ => {}
        }
    }
    text
}

fn trim_tail(mut found: &str) -> &str {
    while let Some(last) = found.chars().next_back() {
        let unmatched = last == ')' && found.matches(')').count() > found.matches('(').count();
        if !(unmatched || ".,;:!?]}」』。，".contains(last)) {
            break;
        }
        found = &found[..found.len() - last.len_utf8()];
    }
    found
}
