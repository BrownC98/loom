//! `loom ask` 질문의 선택지 꼴 검사. 받은편지함은 표준 꼴 `<질문> (A: <선택지> / B: <선택지>, 권장: A)`에서만 선택 단추를 만든다.
//! 선택지를 다른 꼴로 쓰면 단추가 없다(2026-10-23 사람 요청). 문법은 `web/src/choices.ts`와 같고, `choices.cases.json`을 양쪽 시험이 함께 읽는다.

#[derive(PartialEq, Debug)]
struct Choice {
    key: char,
    label: String,
    recommended: bool,
}

#[derive(PartialEq)]
enum Tail {
    None,
    Recommended,
    Mixed,
}

/// 끝 조각 `권장: A`를 읽었으면 `Tail::Recommended`, 마지막 쉼표 뒤가 「권장」을 말하는데 그 꼴이 아니면 `Tail::Mixed`다(선택지 글에 섞여 든다).
fn parse(question: &str) -> Option<(Vec<Choice>, Tail)> {
    let inside = last_group(question.trim())?;
    let commas = top_level(inside, ",");
    let last = commas.last().map(|&cut| (cut, &inside[cut + 1..]));
    let (list, tail, star) = match last {
        Some((cut, part)) if recommended_key(part).is_some() => {
            (&inside[..cut], Tail::Recommended, recommended_key(part))
        }
        Some((_, part)) if mentions_recommendation(part) => (inside, Tail::Mixed, None),
        _ => (inside, Tail::None, None),
    };
    let options: Option<Vec<(char, String)>> =
        split(list, " / ").iter().map(|p| option(p)).collect();
    let options = options.filter(|o| o.len() >= 2)?;
    let choices = options
        .into_iter()
        .map(|(key, label)| Choice {
            recommended: Some(key) == star,
            key,
            label,
        })
        .collect();
    Some((choices, tail))
}

/// 질문이 선택지를 내놓는 듯한데(`A`·`B` 글자나 「권장」 낱말) 단추가 뜨는 표준 꼴이 아니면 참이다. 예·아니오 질문은 거짓이다.
pub fn offers_choices_without_tail(question: &str) -> bool {
    match parse(question) {
        Some((_, tail)) => tail == Tail::Mixed,
        None => {
            !question.trim().is_empty()
                && (label_letters(question) >= 2 || recommendation_near_letter(question))
        }
    }
}

fn mentions_recommendation(text: &str) -> bool {
    text.contains("권장") || text.to_lowercase().contains("recommend")
}

/// 「권장」 낱말 가까이(앞뒤 8글자)에 선택지 글자가 서 있으면 참이다: `(A, 권장)`, `권장 A`. 「권장 설정」 같은 보통 글은 거짓이다.
fn recommendation_near_letter(text: &str) -> bool {
    let original: Vec<char> = text.chars().collect();
    let word_at = |i: usize, word: &str| {
        let len = word.chars().count();
        original.len() >= i + len
            && original[i..i + len]
                .iter()
                .zip(word.chars())
                .all(|(a, b)| a.to_ascii_lowercase() == b)
    };
    (0..original.len())
        .filter(|&i| word_at(i, "권장") || word_at(i, "recommend"))
        .any(|i| {
            let (from, to) = (i.saturating_sub(8), (i + 8).min(original.len()));
            (from..to).any(|j| {
                original[j].is_ascii_uppercase()
                    && (j == 0 || !original[j - 1].is_alphanumeric())
                    && original.get(j + 1).is_none_or(|n| !n.is_alphanumeric())
            })
        })
}

/// 낱말 밖에 선 대문자 한 글자 `A:`의 서로 다른 글자 수. `C:\`는 경로라 뺀다.
fn label_letters(text: &str) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut seen = Vec::new();
    for (i, &c) in chars.iter().enumerate() {
        let alone = c.is_ascii_uppercase() && (i == 0 || !chars[i - 1].is_alphanumeric());
        let marked = match (chars.get(i + 1), chars.get(i + 2)) {
            (Some(':'), Some('\\' | '/')) => false,
            (Some(':'), _) => true,
            _ => false,
        };
        if alone && marked && !seen.contains(&c) {
            seen.push(c);
        }
    }
    seen.len()
}

fn option(part: &str) -> Option<(char, String)> {
    let part = part.trim();
    let mut chars = part.chars();
    let key = chars.next().filter(char::is_ascii_uppercase)?;
    let label = chars.as_str().strip_prefix(':')?.trim_start();
    // 화면 정규식 `.+`는 줄바꿈에서 끊긴다.
    (!label.is_empty() && !label.contains('\n')).then(|| (key, label.trim().to_owned()))
}

/// `<낱말>: A` 꼴인 끝 조각의 `A`. 「권장」 낱말은 사람의 언어라 무엇이든 받는다.
fn recommended_key(part: &str) -> Option<char> {
    let (word, key) = part.trim().split_once(':')?;
    let mut key = key.trim().chars();
    let first = key.next().filter(char::is_ascii_uppercase)?;
    (!word.is_empty() && key.next().is_none()).then_some(first)
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
        options: Vec<Expected>,
    }

    #[derive(serde::Deserialize)]
    struct Expected {
        key: char,
        label: String,
        recommended: bool,
    }

    #[test]
    fn parser_agrees_with_the_shared_cases() {
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("../web/src/choices.cases.json")).unwrap();
        for case in cases {
            let want: Vec<Choice> = case
                .options
                .into_iter()
                .map(|e| Choice {
                    key: e.key,
                    label: e.label,
                    recommended: e.recommended,
                })
                .collect();
            let got = parse(&case.question).map(|(c, _)| c).unwrap_or_default();
            assert_eq!(got, want, "{}", case.question);
        }
    }

    #[test]
    fn standard_tail_and_yes_no_pass() {
        for ok in [
            "승인할까요? (A: 승인 / B: 수정, 권장: A)",
            "지울까요? (A: 지금 / B: 나중)",
            "배포할까요?",
            "이 방식으로 계속할까요? 아니면 멈출까요",
            "권장 설정으로 설치할까요?",
            "권장안대로 진행할까요?",
            "Apply the recommended settings?",
            "[권장안 보고서](loom:doc)를 승인할까요?",
            "A, B, C 노드를 지울까요?",
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
            "잰다(A, 권장) / 다시 학습한다(B)",
            "Pick? (A: x\ny / B: z)",
            "Pick one (A: now / B: later) please",
            "Which? (A: now / B: later, Recommended is A)",
        ] {
            assert!(offers_choices_without_tail(bad), "{bad}");
        }
    }
}
