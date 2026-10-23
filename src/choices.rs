use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 질문의 선택지 하나. `key`가 답으로 가는 글이다.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Choice {
    pub key: char,
    pub label: String,
    pub recommended: bool,
}

pub fn choices(question: &str) -> Vec<Choice> {
    parse(question).map(|(c, _)| c).unwrap_or_default()
}

#[derive(PartialEq)]
enum Tail {
    None,
    Recommended,
    Mixed,
}

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

pub fn offers_choices_without_tail(question: &str) -> bool {
    match parse(question) {
        Some((_, tail)) => tail == Tail::Mixed,
        None => {
            !question.trim().is_empty() && (label_letters(question) >= 2 || tail_shape(question))
        }
    }
}

fn mentions_recommendation(text: &str) -> bool {
    text.contains("권장") || text.to_lowercase().contains("recommend")
}

fn tail_shape(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let key_at = |j: usize| {
        chars.get(j).is_some_and(char::is_ascii_uppercase)
            && chars.get(j + 1).is_none_or(|n| !n.is_alphanumeric())
    };
    let skip = |mut j: usize, pred: fn(&char) -> bool| {
        while chars.get(j).is_some_and(pred) {
            j += 1;
        }
        j
    };
    let word_len = |i: usize| {
        ["권장", "recommend"].iter().find_map(|word| {
            let len = word.chars().count();
            let hit = chars.len() >= i + len
                && chars[i..i + len]
                    .iter()
                    .zip(word.chars())
                    .all(|(a, b)| a.to_ascii_lowercase() == b);
            hit.then_some(len)
        })
    };
    (0..chars.len()).any(|i| {
        let Some(len) = word_len(i) else {
            return false;
        };
        let after = skip(skip(i + len, |c| c.is_alphabetic()), |c| *c == ' ');
        let colon_key = chars.get(after) == Some(&':') && key_at(skip(after + 1, |c| *c == ' '));
        let before = {
            let mut j = i;
            while j > 0 && chars[j - 1] == ' ' {
                j -= 1;
            }
            j
        };
        let paren_key = before >= 3
            && chars[before - 1] == ','
            && chars[before - 2].is_ascii_uppercase()
            && chars[before - 3] == '(';
        colon_key || paren_key
    })
}

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
    (!label.is_empty() && !label.contains(['\n', '\r', '\u{2028}', '\u{2029}']))
        .then(|| (key, label.trim().to_owned()))
}

fn recommended_key(part: &str) -> Option<char> {
    let (word, key) = part.trim().split_once(':')?;
    let mut key = key.trim().chars();
    let first = key.next().filter(char::is_ascii_uppercase)?;
    (!word.is_empty() && key.next().is_none()).then_some(first)
}

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
        let cases: Vec<Case> = serde_json::from_str(include_str!("choices.cases.json")).unwrap();
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
            assert_eq!(choices(&case.question), want, "{}", case.question);
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
            "May I recommend a retry?",
            "I recommend merging now. OK?",
            "I recommend the default. Proceed?",
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
