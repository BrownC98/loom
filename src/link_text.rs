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
            None => &link[1..],
        };
    }
    bare_in(rest)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct Case {
        text: String,
        refused: bool,
        links: Vec<String>,
    }

    fn link_targets(text: &str) -> Vec<String> {
        parts(text)
            .into_iter()
            .filter_map(|part| match part {
                TextPart::Text { .. } => None,
                TextPart::Link { href, .. } => Some(href),
                TextPart::Doc { .. } => Some(DOC_LINK.to_owned()),
            })
            .collect()
    }

    #[test]
    fn accepted_text_draws_every_link() {
        let cases: Vec<Case> = serde_json::from_str(include_str!("link_text.cases.json")).unwrap();
        for case in cases {
            assert_eq!(
                bare_link(&case.text).is_some(),
                case.refused,
                "{}",
                case.text
            );
            if !case.refused {
                assert_eq!(link_targets(&case.text), case.links, "{}", case.text);
            }
        }
    }

    #[test]
    fn parts_split_out_only_drawn_links() {
        let text = |t: &str| TextPart::Text { text: t.into() };
        assert_eq!(
            parts("see [b.html](file:///home/a/%ED%98%91/b.html) now"),
            [
                text("see "),
                TextPart::Link {
                    label: "b.html".into(),
                    href: "file:///home/a/%ED%98%91/b.html".into()
                },
                text(" now"),
            ]
        );
        assert_eq!(parts("no link [x] (y)"), [text("no link [x] (y)")]);
        assert_eq!(
            parts("[x](javascript:alert(1))"),
            [text("[x](javascript:alert(1))")]
        );
        assert_eq!(parts("[x](<https://a.io>)"), [text("[x](<https://a.io>)")]);
        assert_eq!(
            parts("open [R9 tool](loom:doc), then pick"),
            [
                text("open "),
                TextPart::Doc {
                    label: "R9 tool".into()
                },
                text(", then pick"),
            ]
        );
        assert_eq!(parts(""), []);
    }

    #[test]
    fn path_links_name_the_place_inside_the_task_folder() {
        assert_eq!(
            path_link("/home/me/.cache/loom/출력표준2/review/opus.md"),
            TextPart::Link {
                label: "review/opus.md".into(),
                href: "file:///home/me/.cache/loom/%EC%B6%9C%EB%A0%A5%ED%91%9C%EC%A4%802/review/opus.md".into()
            }
        );
        assert_eq!(
            path_link("C:\\Users\\me\\.cache\\loom\\t\\logs\\merge.log"),
            TextPart::Link {
                label: "logs/merge.log".into(),
                href: "file:///C:/Users/me/.cache/loom/t/logs/merge.log".into()
            }
        );
        for (path, href) in [
            ("/tmp/result#1.md", "file:///tmp/result%231.md"),
            ("/tmp/result?1.md", "file:///tmp/result%3F1.md"),
            ("/tmp/result%20.md", "file:///tmp/result%2520.md"),
            ("/tmp/result%.md", "file:///tmp/result%25.md"),
            ("file:///tmp/a%231.md", "file:///tmp/a%231.md"),
            ("C:\\work\\result#1%.md", "file:///C:/work/result%231%25.md"),
        ] {
            let TextPart::Link { href: got, .. } = path_link(path) else {
                panic!("{path}");
            };
            assert_eq!(got, href);
            let decoded = percent_encoding::percent_decode_str(&got).decode_utf8_lossy();
            if !is_url(path) {
                assert!(decoded.ends_with(&path.replace('\\', "/")), "{decoded}");
            }
        }
        assert_eq!(
            path_link("/tmp/a b.md"),
            TextPart::Link {
                label: "/tmp/a b.md".into(),
                href: "file:///tmp/a%20b.md".into()
            }
        );
    }

    #[test]
    fn refused_targets_are_found_whole() {
        for (bad, found) in [
            (
                "작성: https://docs.google.com/a/b.",
                "https://docs.google.com/a/b",
            ),
            ("[x](https://a.io) 그리고 http://b.io/c,", "http://b.io/c"),
            ("파일은 /home/a/b.md 에 있다", "/home/a/b.md"),
            ("(/home/a/b.md)", "/home/a/b.md"),
            ("`/tmp/a`", "/tmp/a"),
            ("path=/tmp/a", "/tmp/a"),
            ("`/tmp/my dir/a`", "/tmp/my dir/a"),
            ("POST /api/nodes", "/api/nodes"),
            ("C:\\Users\\a\\b.md", "C:\\Users\\a\\b.md"),
            ("\"C:\\Program Files\\a.txt\"", "C:\\Program Files\\a.txt"),
            ("\\\\host\\share\\a.txt", "\\\\host\\share\\a.txt"),
            ("file:///home/a/b.md", "file:///home/a/b.md"),
            ("_https://a.io_", "https://a.io_"),
            ("-https://a.io", "https://a.io"),
            ("`https://example.com/a(b)`", "https://example.com/a(b)"),
            ("\"/tmp/a(b)\"", "/tmp/a(b)"),
            ("https://example.com/a(b).", "https://example.com/a(b)"),
            ("[r](/home/a/b.md)", "/home/a/b.md"),
            ("[x](<https://a.io/b>)", "https://a.io/b"),
            ("[x](https://a.io/a(b)/c)", "https://a.io/a(b)/c"),
            ("[보고서](/home/a/b.md)를 열까요?", "/home/a/b.md"),
        ] {
            assert_eq!(bare_link(bad).as_deref(), Some(found), "{bad}");
        }
    }

    #[test]
    fn repair_form_is_a_drawn_target() {
        for (found, form) in [
            ("https://a.io/a(b)/c", "https://a.io/a%28b%29/c"),
            ("/tmp/my dir/a(1).md", "file:///tmp/my%20dir/a%281%29.md"),
            (
                "C:\\Program Files\\a.txt",
                "file:///C:/Program%20Files/a.txt",
            ),
            ("\\\\host\\share\\a.txt", "file://host/share/a.txt"),
            ("https://a.io/a(b)", "https://a.io/a%28b%29"),
        ] {
            let form_out = drawable_form(found);
            assert_eq!(form_out, form);
            assert_eq!(bare_link(&format!("[x]({form_out})")), None);
        }
    }

    #[test]
    fn bracket_runs_within_the_one_kilobyte_bound_stay_fast() {
        assert_eq!(bare_link(&"[".repeat(1_000)), None);
        assert_eq!(bare_link(&"[a](".repeat(250)), None);
    }
}
