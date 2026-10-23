//! `loom ask` 질문의 선택지 꼴 검사. 받은편지함은 표준 꼴 `<질문> (A: <선택지> / B: <선택지>, 권장: A)`에서만 선택 단추를 만든다.
//! 선택지를 다른 꼴로 쓰면 단추가 없다(2026-10-23 사람 요청). 문법은 `web/src/choices.ts`와 같다. `choices.cases.json`의 예를 양쪽 시험이 함께 읽는다.

/// 표준 꼴의 선택지 글자(`A`, `B`…). 꼴이 아니거나 선택지가 둘보다 적으면 빈 목록이다.
#[cfg(test)]
fn option_keys(question: &str) -> Vec<char> {
    parse(question).map(|(keys, _)| keys).unwrap_or_default()
}

/// (선택지 글자, 끝 조각의 모양). 끝 조각 `권장: A`를 읽었으면 `Tail::Recommended`, 마지막 쉼표 뒤가 선택지에 섞여 들었으면 `Tail::Mixed`다.
fn parse(question: &str) -> Option<(Vec<char>, Tail)> {
    let inside = last_group(question.trim())?;
    let commas = top_level(inside, ",");
    let last = commas.last().map(|&cut| (cut, &inside[cut + 1..]));
    let (list, tail) = match last {
        Some((cut, part)) if recommended_tail(part) => (&inside[..cut], Tail::Recommended),
        Some((_, part)) if mentions_recommendation(part) => (inside, Tail::Mixed),
        _ => (inside, Tail::None),
    };
    let keys: Option<Vec<char>> = split(list, " / ").iter().map(|p| option_key(p)).collect();
    keys.filter(|k| k.len() >= 2).map(|k| (k, tail))
}

#[derive(PartialEq)]
enum Tail {
    None,
    Recommended,
    Mixed,
}

/// 질문이 선택지를 내놓는 듯한데(`A`·`B` 글자나 「권장」 낱말) 단추가 뜨는 표준 꼴이 아니면 참이다. 예·아니오 질문은 거짓이다.
pub fn offers_choices_without_tail(question: &str) -> bool {
    match parse(question) {
        Some((_, tail)) => tail == Tail::Mixed,
        None => {
            !question.trim().is_empty()
                && (mentions_recommendation(question) || label_letters(question) >= 2)
        }
    }
}

fn mentions_recommendation(text: &str) -> bool {
    text.contains("권장") || text.to_lowercase().contains("recommend")
}

/// 낱말 밖에 선 대문자 한 글자 `A:` `A,` `A)`의 서로 다른 글자 수. `C:\`는 경로라 뺀다.
fn label_letters(text: &str) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut seen = Vec::new();
    for (i, &c) in chars.iter().enumerate() {
        let alone = c.is_ascii_uppercase() && (i == 0 || !chars[i - 1].is_alphanumeric());
        let marked = match (chars.get(i + 1), chars.get(i + 2)) {
            (Some(':'), Some('\\' | '/')) => false,
            (Some(':' | ',' | ')'), _) => true,
            _ => false,
        };
        if alone && marked && !seen.contains(&c) {
            seen.push(c);
        }
    }
    seen.len()
}

/// `A: 글`이면 `A`.
fn option_key(part: &str) -> Option<char> {
    let part = part.trim();
    let mut chars = part.chars();
    let key = chars.next().filter(char::is_ascii_uppercase)?;
    let label = chars.as_str().strip_prefix(':')?;
    (!label.trim().is_empty()).then_some(key)
}

/// `<낱말>: A` 꼴인 끝 조각. 「권장」 낱말은 사람의 언어라 무엇이든 받는다.
fn recommended_tail(part: &str) -> bool {
    part.trim().split_once(':').is_some_and(|(word, key)| {
        let mut key = key.trim().chars();
        !word.is_empty()
            && key.next().is_some_and(|c| c.is_ascii_uppercase())
            && key.next().is_none()
    })
}

/// 글 끝의 맨 바깥 `(...)` 안쪽.
fn last_group(text: &str) -> Option<&str> {
    if !text.ends_with(')') {
        return None;
    }
    let mut depth = 0i32;
    for (i, c) in text.char_indices().rev() {
        match c {
            ')' => depth += 1,
            '(' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[i + 1..text.len() - 1]);
                }
            }
            _ => {}
        }
    }
    None
}

/// 괄호 밖에 있는 `sep`의 시작 위치들.
fn top_level(text: &str, sep: &str) -> Vec<usize> {
    let mut at = Vec::new();
    let mut depth = 0i32;
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ if depth == 0 && text[i..].starts_with(sep) => at.push(i),
            _ => {}
        }
    }
    at
}

fn split<'a>(text: &'a str, sep: &str) -> Vec<&'a str> {
    let mut parts = Vec::new();
    let mut from = 0;
    for i in top_level(text, sep) {
        parts.push(&text[from..i]);
        from = i + sep.len();
    }
    parts.push(&text[from..]);
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct Case {
        question: String,
        keys: String,
    }

    #[test]
    fn parser_agrees_with_the_shared_cases() {
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("../web/src/choices.cases.json")).unwrap();
        for case in cases {
            let keys: String = option_keys(&case.question).into_iter().collect();
            assert_eq!(keys, case.keys, "{}", case.question);
        }
    }

    #[test]
    fn standard_tail_and_yes_no_pass() {
        for ok in [
            "승인할까요? (A: 승인 / B: 수정, 권장: A)",
            "지울까요? (A: 지금 / B: 나중)",
            "배포할까요?",
            "이 방식으로 계속할까요? 아니면 멈출까요",
            "C:\\temp 폴더를 지워도 될까요?",
        ] {
            assert!(!offers_choices_without_tail(ok), "{ok}");
        }
    }

    #[test]
    fn inline_options_and_bad_tails_are_refused() {
        for bad in [
            "정확도를 잰다(A, 권장) / 다시 학습한다(B)",
            "어느 쪽으로 할까요? A: 지금 B: 나중",
            "어느 쪽으로 할까요? (A: 지금 / B: 나중, 권장 A)",
            "이쪽이 권장입니다. 계속할까요?",
            "Pick one (A: now / B: later) please",
            "Which? (A: now / B: later, Recommended is A)",
        ] {
            assert!(offers_choices_without_tail(bad), "{bad}");
        }
    }
}
