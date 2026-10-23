//! 에이전트가 사람에게 쓰는 글(`loom ask`, 부모 없는 노드의 `loom done`)의 링크 표준: 주소와 절대 경로는 화면이 그리는 markdown 링크 `[이름](주소)` 안에만 둔다.
//! 근거: 2026-10-23 사람 "링크 표준이 없는 것이 뿌리 원인". 화면은 글을 나누지 않고 `parts`가 나눈 조각을 그린다. 그래서 받는 링크와 그리는 링크가 같다.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

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

/// 글을 글 조각과 화면이 그리는 링크로 나눈다. 그리지 못하는 링크는 글로 남는다.
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

/// 경로 칸(결과 파일, 결함 파일, 로그)의 링크. 작업 폴더(`~/.cache/loom/<작업>/`) 안이면 이름은 그 안의 경로다(목업 M17 ① `review/opus.md`).
pub fn path_link(path: &str) -> TextPart {
    let slashed = path.replace('\\', "/");
    let label = slashed
        .split_once("/.cache/loom/")
        .and_then(|(_, task)| task.split_once('/'))
        .map_or(path, |(_, inside)| inside);
    TextPart::Link {
        label: label.to_owned(),
        href: drawable_form(path),
    }
}

pub fn parse_link(link: &str) -> Option<(&str, &str, usize)> {
    let (label, after) = link[1..].split_once("](")?;
    // CommonMark는 주소를 `<…>`로 감쌀 수 있고, 감싼 주소에는 빈칸이 들 수 있다(2026-10-23 검토 BLOCK).
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

/// 본문 속 문서 링크의 주소 표지. 화면은 이 자리(`TextPart::Doc`)에 붙은 문서의 링크를 그린다.
/// 근거: 2026-10-23 사람 지시 "질문 목업 링크도 아래 완료의 링크처럼 본문에 하이퍼 링크 달리는 방식으로 만들어줘".
pub const DOC_LINK: &str = "loom:doc";

/// 화면이 그릴 수 없는 링크 주소나 markdown 링크 밖의 맨몸 주소·경로를 처음 나온 것 하나 돌려준다. 없으면 `None`.
/// `parts`는 `file://`·`http(s)://` 주소(빈칸·`)` 없음)와 문서 링크 표지만 링크로 나눈다.
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
            // 주소는 첫 `)`에서 끊기므로, 짝이 맞는 괄호가 든 주소는 그 뒤 글까지 이어 보여야 온전하다.
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

/// 거절 글에 보일 고친 주소: 경로는 `file://` 주소로, 화면이 그리지 못하는 글자는 퍼센트 인코딩으로 바꾼다.
pub fn drawable_form(found: &str) -> String {
    let url = if ["file://", "http://", "https://"]
        .iter()
        .any(|s| found.starts_with(s))
    {
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

/// 이 앞 글자가 낱말 안이면(`and/or`, `경로/home`) 경로의 시작이 아니다.
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
        // 한 단계 `/code-review`는 슬래시 명령·태그와 구별되지 않아 뺀다. 경로는 `/a/b`부터다.
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

/// 첫 빈칸이나 닫는 기호까지. 끝의 문장부호는 뺀다. 빈칸이 든 경로는 닫는 기호(`` ` `` `"` `'`)가 있으면 거기까지 잡는다.
fn span(tail: &str) -> String {
    let cut = tail.find(['\n', '`', '"', '\'', '<', '>']);
    let quoted = cut.is_some_and(|p| matches!(tail.as_bytes()[p], b'`' | b'"' | b'\''));
    let found = &tail[..cut.unwrap_or(tail.len())];
    if quoted {
        return found.to_owned();
    }
    trim_tail(found.split(char::is_whitespace).next().unwrap_or(found)).to_owned()
}

/// 글 앞에서 닫는 `)`(짝이 맞는 괄호 밖)나 빈칸 앞까지.
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
                href: "file:///home/me/.cache/loom/출력표준2/review/opus.md".into()
            }
        );
        assert_eq!(
            path_link("C:\\Users\\me\\.cache\\loom\\t\\logs\\merge.log"),
            TextPart::Link {
                label: "logs/merge.log".into(),
                href: "file:///C:/Users/me/.cache/loom/t/logs/merge.log".into()
            }
        );
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
