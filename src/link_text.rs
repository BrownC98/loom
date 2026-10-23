//! 에이전트가 사람에게 쓰는 글(`loom ask`, 부모 없는 노드의 `loom done`)의 링크 표준: 주소와 절대 경로는 markdown 링크 `[이름](주소)` 안에만 둔다.
//! 근거: 2026-10-23 사람 "링크 표준이 없는 것이 뿌리 원인". 화면(`web/src/links.ts`)은 markdown 링크만 그린다.

/// `[이름](주소)`로 시작하는 글에서 (이름, 주소, 전체 길이)를 읽는다. 링크가 아니면 `None`.
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
            // 짝이 맞는 괄호는 주소에 든다: `https://a.io/x_(y)`.
            let mut depth = 0usize;
            let close = after.char_indices().find(|&(_, c)| match c {
                '(' => {
                    depth += 1;
                    false
                }
                ')' if depth == 0 => true,
                ')' => {
                    depth -= 1;
                    false
                }
                _ => false,
            })?;
            let target = &after[..close.0];
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

/// markdown 링크 밖에서 처음 나온 맨몸 주소·경로를 링크 주소로 쓸 만큼 돌려준다. 없으면 `None`.
pub fn bare_link(text: &str) -> Option<String> {
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let (before, link) = rest.split_at(open);
        if let Some(found) = bare_in(before) {
            return Some(found);
        }
        rest = match parse_link(link) {
            Some((_, _, len)) => &link[len..],
            None => &link[1..],
        };
    }
    bare_in(rest)
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
        let next = chars.get(i + 1).map(|&(_, n)| n);
        let before = i.checked_sub(1).map(|j| chars[j].1);
        let url = ["http://", "https://", "file://"]
            .iter()
            .any(|s| tail.starts_with(s));
        let unix = c == '/' && next.is_some_and(|n| n != '/' && !n.is_whitespace());
        let drive = c.is_ascii_alphabetic()
            && tail[1..].starts_with(':')
            && (tail[2..].starts_with('\\') || tail[2..].starts_with('/'));
        let unc = tail.starts_with("\\\\") && next == Some('\\') && chars.get(i + 2).is_some();
        (starts_token(before) && (url || unix || drive || unc)).then(|| span(tail))
    })
}

/// 첫 빈칸이나 닫는 기호까지. 끝의 문장부호는 뺀다. 빈칸이 든 경로는 닫는 기호(`` ` `` `"` `'`)가 있으면 거기까지 잡는다.
fn span(tail: &str) -> String {
    let cut = tail.find(['\n', '`', '"', '\'', '<', '>']);
    let quoted = cut.is_some_and(|p| matches!(tail.as_bytes()[p], b'`' | b'"' | b'\''));
    let found = &tail[..cut.unwrap_or(tail.len())];
    let found = if quoted {
        found
    } else {
        found.split(char::is_whitespace).next().unwrap_or(found)
    };
    found
        .trim_end_matches([
            '.', ',', ';', ':', '!', '?', ')', ']', '}', '」', '』', '。', '，',
        ])
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_links_and_plain_text_pass() {
        for ok in [
            "[시트](https://docs.google.com/a/b) 열기.",
            "[보고서](/home/a/b.md)와 [x](file:///home/a/b.md)",
            "[a](<C:\\my dir\\b.md>) 승인? (A / B)",
            "[x](https://example.com/a(b)/c/d) 끝",
            "and/or 선택, src/cli.rs 고침, 승인 / 거절, 경로/home",
            "[깨진 링크 https",
        ] {
            assert_eq!(bare_link(ok), None, "{ok}");
        }
    }

    #[test]
    fn bare_urls_and_paths_are_found_whole() {
        for (bad, found) in [
            (
                "작성: https://docs.google.com/a/b.",
                "https://docs.google.com/a/b",
            ),
            ("[x](https://a.io) 그리고 http://b.io/c,", "http://b.io/c"),
            ("파일은 /home/a/b.md 에 있다", "/home/a/b.md"),
            ("(/home/a/b.md)", "/home/a/b.md"),
            ("한 단계 /tmp 도", "/tmp"),
            ("`/tmp/a`", "/tmp/a"),
            ("path=/tmp/a", "/tmp/a"),
            ("`/tmp/my dir/a`", "/tmp/my dir/a"),
            ("C:\\Users\\a\\b.md", "C:\\Users\\a\\b.md"),
            ("\"C:\\Program Files\\a.txt\"", "C:\\Program Files\\a.txt"),
            ("\\\\host\\share\\a.txt", "\\\\host\\share\\a.txt"),
            ("file:///home/a/b.md", "file:///home/a/b.md"),
            ("`https://a.io/x`", "https://a.io/x"),
        ] {
            assert_eq!(bare_link(bad).as_deref(), Some(found), "{bad}");
        }
    }
}
