#![cfg(unix)]

use std::path::Path;
use std::process::Command;

use tree_sitter::{Language, Node, Parser};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lang {
    Rust,
    TypeScript,
    Tsx,
    Bash,
    PowerShell,
    Yaml,
    Toml,
    Kotlin,
    Xml,
    Css,
    Html,
    Nsis,
    HashLines,
    Properties,
}

enum Kind {
    Code(Lang),
    Unchecked,
}

const UNCHECKED_DIRS: &[&str] = &["docs/", "web/src/gen/"];

const UNCHECKED_NAMES: &[&str] = &["LICENSE", "Cargo.lock", "package-lock.json"];

const UNCHECKED_EXTENSIONS: &[&str] = &["md", "json", "webmanifest", "txt", "png", "ico", "woff2"];

fn kind_of(path: &str) -> Option<Kind> {
    if UNCHECKED_DIRS.iter().any(|dir| path.starts_with(dir)) {
        return Some(Kind::Unchecked);
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    if UNCHECKED_NAMES.contains(&name) {
        return Some(Kind::Unchecked);
    }
    let lang = match name {
        ".gitignore" | ".gitattributes" | ".nvmrc" => Lang::HashLines,
        _ => match name.rsplit_once('.')?.1 {
            "rs" => Lang::Rust,
            "ts" | "mts" | "cts" => Lang::TypeScript,
            "tsx" | "js" | "mjs" | "cjs" | "jsx" => Lang::Tsx,
            "sh" => Lang::Bash,
            "ps1" => Lang::PowerShell,
            "yml" | "yaml" => Lang::Yaml,
            "toml" => Lang::Toml,
            "kt" | "kts" => Lang::Kotlin,
            "xml" => Lang::Xml,
            "css" => Lang::Css,
            "html" => Lang::Html,
            "nsh" | "nsi" => Lang::Nsis,
            "properties" => Lang::Properties,
            ext if UNCHECKED_EXTENSIONS.contains(&ext) => return Some(Kind::Unchecked),
            _ => return None,
        },
    };
    Some(Kind::Code(lang))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verdict {
    Allowed(&'static str),
    HumanAsked(u32),
    Refused,
}

#[derive(Debug)]
struct Comment {
    start: usize,
    end: usize,
    verdict: Verdict,
}

fn grammar(lang: Lang) -> Language {
    match lang {
        Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
        Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        Lang::Bash => tree_sitter_bash::LANGUAGE.into(),
        Lang::PowerShell => tree_sitter_powershell::LANGUAGE.into(),
        Lang::Yaml => tree_sitter_yaml::LANGUAGE.into(),
        Lang::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
        Lang::Kotlin => tree_sitter_kotlin_sg::LANGUAGE.into(),
        Lang::Xml => tree_sitter_xml::LANGUAGE_XML.into(),
        Lang::Css => tree_sitter_css::LANGUAGE.into(),
        Lang::Html => tree_sitter_html::LANGUAGE.into(),
        Lang::Nsis | Lang::HashLines | Lang::Properties => unreachable!("line formats"),
    }
}

fn comments(lang: Lang, text: &str) -> Result<Vec<Comment>, String> {
    let spans = match lang {
        Lang::HashLines => line_comments(text, |line| line.starts_with('#')),
        Lang::Properties => line_comments(text, |line| line.trim_start().starts_with(['#', '!'])),
        Lang::Nsis => nsis_comments(text),
        _ => {
            let mut found = Vec::new();
            tree_comments(lang, text, 0, &mut found)?;
            return Ok(found);
        }
    };
    Ok(spans
        .into_iter()
        .map(|(start, end)| Comment {
            start,
            end,
            verdict: judge(lang, &text[start..end], start == 0, None),
        })
        .collect())
}

fn tree_comments(
    lang: Lang,
    text: &str,
    offset: usize,
    found: &mut Vec<Comment>,
) -> Result<(), String> {
    let mut parser = Parser::new();
    parser
        .set_language(&grammar(lang))
        .map_err(|e| e.to_string())?;
    let tree = parser
        .parse(text, None)
        .ok_or_else(|| "parser gave no tree".to_owned())?;
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.is_error() || node.is_missing() {
            let line = text[..node.start_byte()].matches('\n').count() + 1;
            return Err(format!(
                "{lang:?} parser cannot read line {line}; fix the syntax or the file kind"
            ));
        }
        if node.kind().ends_with("comment") || node.kind() == "Comment" {
            let (start, end) = (node.start_byte(), node.end_byte());
            let end = start + text[start..end].trim_end().len();
            let cut = match node.parent() {
                Some(p) if p.kind() == "jsx_expression" && p.named_child_count() == 1 => {
                    p.byte_range()
                }
                _ => start..end,
            };
            found.push(Comment {
                start: offset + cut.start,
                end: offset + cut.end,
                verdict: judge(
                    lang,
                    &text[start..end],
                    offset + start == 0,
                    Some((node, text)),
                ),
            });
            continue;
        }
        if lang == Lang::Html && node.kind() == "raw_text" {
            let inner = match node.parent().map(|p| p.kind()) {
                Some("script_element") => Some(Lang::Tsx),
                Some("style_element") => Some(Lang::Css),
                _ => None,
            };
            if let Some(inner) = inner {
                let range = node.byte_range();
                tree_comments(inner, &text[range.clone()], offset + range.start, found)?;
                continue;
            }
        }
        let mut cursor = node.walk();
        stack.extend(node.children(&mut cursor));
    }
    found.sort_by_key(|c| c.start);
    Ok(())
}

fn line_comments(text: &str, is_comment: impl Fn(&str) -> bool) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end();
        if is_comment(body) {
            let lead = body.len() - body.trim_start().len();
            spans.push((at + lead, at + body.len()));
        }
        at += line.len();
    }
    spans
}

fn nsis_comments(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    let mut token_start = true;
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'"' | b'\'' | b'`') => {
                i += 1;
                while i < bytes.len() && bytes[i] != quote && bytes[i] != b'\n' {
                    i += if bytes[i] == b'$' && bytes.get(i + 1) == Some(&b'\\') {
                        3
                    } else {
                        1
                    };
                }
                i += 1;
                token_start = false;
            }
            b';' | b'#' if token_start => {
                let start = i;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                spans.push((start, start + text[start..i].trim_end().len()));
            }
            b'/' if token_start && bytes.get(i + 1) == Some(&b'*') => {
                let start = i;
                i = text[i + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |e| i + 2 + e + 2);
                spans.push((start, i));
            }
            c => {
                token_start = c.is_ascii_whitespace();
                i += 1;
            }
        }
    }
    spans
}

const DIRECTIVES: &[(&[Lang], &str, &str)] = &[
    (&[Lang::TypeScript, Lang::Tsx], "@ts-expect-error", "tsc"),
    (&[Lang::TypeScript, Lang::Tsx], "@ts-ignore", "tsc"),
    (&[Lang::TypeScript, Lang::Tsx], "@ts-nocheck", "tsc"),
    (&[Lang::TypeScript, Lang::Tsx], "@ts-check", "tsc"),
    (&[Lang::TypeScript, Lang::Tsx], "<reference ", "tsc"),
    (&[Lang::TypeScript, Lang::Tsx], "eslint-disable", "eslint"),
    (&[Lang::TypeScript, Lang::Tsx], "eslint-enable", "eslint"),
    (&[Lang::Bash], "shellcheck ", "shellcheck"),
];

const CLAP_DERIVES: &[&str] = &["Parser", "Subcommand", "Args", "ValueEnum"];

fn judge(lang: Lang, text: &str, at_file_start: bool, node: Option<(Node, &str)>) -> Verdict {
    if at_file_start && text.starts_with("#!") && lang != Lang::Rust {
        return Verdict::Allowed("program loader (shebang)");
    }
    let body = body_of(text);
    if let Some(id) = human_question(body) {
        return Verdict::HumanAsked(id);
    }
    if let Some((_, _, tool)) = DIRECTIVES
        .iter()
        .find(|(langs, prefix, _)| langs.contains(&lang) && body.starts_with(prefix))
    {
        return Verdict::Allowed(tool);
    }
    if lang == Lang::Rust
        && text.starts_with("///")
        && !text.starts_with("////")
        && let Some(tool) = node.and_then(|(n, source)| rust_doc_reader(n, source))
    {
        return Verdict::Allowed(tool);
    }
    Verdict::Refused
}

fn body_of(text: &str) -> &str {
    text.trim_start_matches(['/', '#', ';', '*', '!'])
        .trim_start_matches("<!--")
        .trim()
}

fn human_question(body: &str) -> Option<u32> {
    let digits = body.strip_prefix('Q')?;
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    let id = digits[..end].parse().ok()?;
    match digits[end..].chars().next() {
        Some(c) if c.is_alphanumeric() || c == '_' => None,
        _ => Some(id),
    }
}

fn rust_doc_reader(comment: Node, source: &str) -> Option<&'static str> {
    let skipped = |n: &Node| {
        matches!(
            n.kind(),
            "line_comment" | "block_comment" | "attribute_item"
        )
    };
    let mut item = comment;
    while skipped(&item) {
        item = item.next_named_sibling()?;
    }
    let owner = std::iter::successors(Some(item), |n| n.parent())
        .find(|n| matches!(n.kind(), "struct_item" | "enum_item"))?;
    let attributes: String =
        std::iter::successors(owner.prev_named_sibling(), |n| n.prev_named_sibling())
            .take_while(skipped)
            .filter(|n| n.kind() == "attribute_item")
            .map(|n| &source[n.byte_range()])
            .collect();
    let words: Vec<&str> = attributes
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .collect();
    if !words.contains(&"derive") {
        None
    } else if words.contains(&"TS") {
        Some("ts-rs (copies it into web/src/gen)")
    } else if CLAP_DERIVES.iter().any(|d| words.contains(d)) {
        Some("clap (CLI help)")
    } else {
        None
    }
}

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn files() -> Vec<String> {
    let out = Command::new("git")
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .current_dir(repo())
        .output()
        .expect("git ls-files");
    assert!(out.status.success(), "git ls-files failed");
    String::from_utf8(out.stdout)
        .expect("paths are UTF-8")
        .split('\0')
        .filter(|p| !p.is_empty() && repo().join(p).is_file())
        .map(str::to_owned)
        .collect()
}

fn line_of(text: &str, at: usize) -> usize {
    text[..at].matches('\n').count() + 1
}

fn answered_by_human(id: u32) -> Result<bool, String> {
    let var = |name| std::env::var(name).map_err(|_| format!("{name} is not set"));
    let (url, token) = (var("LOOM_URL")?, var("LOOM_TOKEN")?);
    let mut res = ureq::post(format!("{url}/api/agent/question"))
        .header("authorization", format!("Bearer {token}"))
        .send_json(serde_json::json!({ "id": id }))
        .map_err(|e| e.to_string())?;
    let state: serde_json::Value = res.body_mut().read_json().map_err(|e| e.to_string())?;
    Ok(state["answered"] == true)
}

struct Scan {
    problems: Vec<String>,
    asked: Vec<(String, u32)>,
}

fn scan() -> Scan {
    let mut scan = Scan {
        problems: Vec::new(),
        asked: Vec::new(),
    };
    for path in files() {
        let lang = match kind_of(&path) {
            Some(Kind::Code(lang)) => lang,
            Some(Kind::Unchecked) => continue,
            None => {
                scan.problems.push(format!(
                    "{path}: unknown file kind (tracked or untracked, not ignored); add it to `kind_of` in tests/comments/main.rs"
                ));
                continue;
            }
        };
        let text = std::fs::read_to_string(repo().join(&path)).expect("code is UTF-8");
        let found = match comments(lang, &text) {
            Ok(found) => found,
            Err(e) => {
                scan.problems.push(format!("{path}: {e}"));
                continue;
            }
        };
        for c in found {
            let at = format!("{path}:{}", line_of(&text, c.start));
            match c.verdict {
                Verdict::Allowed(_) => {}
                Verdict::HumanAsked(id) => scan.asked.push((at, id)),
                Verdict::Refused => scan.problems.push(format!("{at}: comment")),
            }
        }
    }
    scan
}

#[test]
fn repo_has_no_refused_comments() {
    let problems = scan().problems;
    assert!(
        problems.is_empty(),
        "Comments are not allowed unless the human asked for them (AGENTS.md). Remove them with `memcap -m 5G cargo test --test comments -- --ignored strip`:\n{}",
        problems.join("\n")
    );
}

#[test]
#[ignore = "asks the loom daemon; scripts/gate/check.sh runs it"]
fn asked_questions_were_answered() {
    let problems: Vec<String> = scan()
        .asked
        .into_iter()
        .filter_map(|(at, id)| match answered_by_human(id) {
            Ok(true) => None,
            Ok(false) => Some(format!(
                "{at}: Q{id} is not a question that the human answered"
            )),
            Err(e) => Some(format!("{at}: cannot ask the loom daemon about Q{id}: {e}")),
        })
        .collect();
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
#[ignore = "rewrites the repo files"]
fn strip() {
    for path in files() {
        let Some(Kind::Code(lang)) = kind_of(&path) else {
            continue;
        };
        let file = repo().join(&path);
        let text = std::fs::read_to_string(&file).expect("code is UTF-8");
        let found = comments(lang, &text).unwrap_or_else(|e| panic!("{path}: {e}"));
        let stripped = without(&text, &found);
        if stripped != text {
            std::fs::write(&file, stripped).expect("write");
        }
    }
}

fn without(text: &str, found: &[Comment]) -> String {
    let mut out = text.to_owned();
    for c in found.iter().rev().filter(|c| c.verdict == Verdict::Refused) {
        let line_start = out[..c.start].rfind('\n').map_or(0, |i| i + 1);
        let line_end = out[c.end..].find('\n').map_or(out.len(), |i| c.end + i);
        let before = &out[line_start..c.start];
        let after = &out[c.end..line_end];
        match (before.trim().is_empty(), after.trim().is_empty()) {
            (true, true) => {
                let next = (line_end + 1).min(out.len());
                let blank_above = line_start == 0 || out[..line_start - 1].ends_with('\n');
                let blank_below = out[next..].starts_with('\n') || next == out.len();
                let next = if blank_above && blank_below && next < out.len() {
                    next + 1
                } else {
                    next
                };
                out.replace_range(line_start..next, "");
            }
            (false, true) => {
                let start = line_start + before.trim_end().len();
                out.replace_range(start..line_end, "");
            }
            (true, false) => {
                let end = line_end - after.trim_start().len();
                out.replace_range(c.start..end, "");
            }
            (false, false) => {
                let end = line_end - after.trim_start().len();
                let joins = |a: char, b: char| {
                    let word = |c: char| c.is_alphanumeric() || c == '_';
                    let operator = |c: char| "+-*/%<>=&|!^:.?".contains(c);
                    (word(a) && word(b)) || (operator(a) && operator(b))
                };
                let gap = match (before.chars().last(), after.trim_start().chars().next()) {
                    (Some(a), Some(b)) if !a.is_whitespace() && joins(a, b) => " ",
                    _ => "",
                };
                out.replace_range(c.start..end, gap);
            }
        }
    }
    out
}

#[cfg(test)]
mod cases {
    use super::*;

    fn verdicts(lang: Lang, text: &str) -> Vec<Verdict> {
        comments(lang, text)
            .unwrap()
            .into_iter()
            .map(|c| c.verdict)
            .collect()
    }

    #[test]
    fn a_plain_comment_is_refused() {
        assert_eq!(
            verdicts(Lang::Rust, "fn a() {} // why\n"),
            [Verdict::Refused]
        );
        assert_eq!(
            verdicts(Lang::TypeScript, "/* why */ const a = 1;\n"),
            [Verdict::Refused]
        );
        assert_eq!(verdicts(Lang::Bash, "#!/bin/sh\necho a # why\n").len(), 2);
        assert_eq!(verdicts(Lang::Yaml, "a: 1 # why\n"), [Verdict::Refused]);
        assert_eq!(verdicts(Lang::Toml, "# why\na = 1\n"), [Verdict::Refused]);
        assert_eq!(
            verdicts(Lang::Html, "<p>a</p><!-- why -->\n"),
            [Verdict::Refused]
        );
        assert_eq!(
            verdicts(Lang::Nsis, "!define A \"x\" ; why\n"),
            [Verdict::Refused]
        );
    }

    #[test]
    fn comment_marks_inside_strings_are_not_comments() {
        assert!(verdicts(Lang::Rust, "const A: &str = r#\"// a /* b */\"#;\n").is_empty());
        assert!(
            verdicts(
                Lang::Tsx,
                "const a = `// ${\"/*\"}`; const b = /\\/\\//g;\nconst c = <p>// text</p>;\n"
            )
            .is_empty()
        );
        assert!(verdicts(Lang::Bash, "echo \"# a\" ${#b} $#\ncat <<EOF\n# c\nEOF\n").is_empty());
        assert!(verdicts(Lang::Yaml, "a: |\n  # b\nc: \"#d\"\n").is_empty());
        assert!(verdicts(Lang::Nsis, "StrCpy $0 \"a ; $\\\"b # c\"\n").is_empty());
    }

    #[test]
    fn a_doc_comment_is_refused_unless_a_tool_reads_it() {
        assert_eq!(
            verdicts(Lang::Rust, "/// why\nfn a() {}\n//! crate\n"),
            [Verdict::Refused, Verdict::Refused]
        );
        assert_eq!(
            verdicts(
                Lang::Rust,
                "/// help\n#[derive(Debug, TS)]\nstruct A {\n    /// field\n    b: u8,\n}\n"
            ),
            [Verdict::Allowed("ts-rs (copies it into web/src/gen)"); 2]
        );
        assert_eq!(
            verdicts(
                Lang::Rust,
                "#[derive(clap::Subcommand)]\nenum Cli {\n    /// help\n    Go {\n        /// arg\n        name: String,\n    },\n}\n"
            ),
            [Verdict::Allowed("clap (CLI help)"); 2]
        );
        assert_eq!(
            verdicts(Lang::TypeScript, "/** why */\nexport const a = 1;\n"),
            [Verdict::Refused]
        );
    }

    #[test]
    fn tool_directives_are_allowed() {
        assert_eq!(
            verdicts(
                Lang::Bash,
                "#!/usr/bin/env bash\n# shellcheck disable=SC2086\n"
            ),
            [
                Verdict::Allowed("program loader (shebang)"),
                Verdict::Allowed("shellcheck"),
            ]
        );
        assert_eq!(
            verdicts(
                Lang::TypeScript,
                "/// <reference types=\"vite/client\" />\n// @ts-expect-error old\nconst a: number = 'b';\n/* eslint-disable no-console */\n"
            ),
            [
                Verdict::Allowed("tsc"),
                Verdict::Allowed("tsc"),
                Verdict::Allowed("eslint")
            ]
        );
        assert_eq!(
            verdicts(Lang::Rust, "#!/usr/bin/env rust\n"),
            Vec::<Verdict>::new()
        );
    }

    #[test]
    fn a_comment_that_starts_with_a_question_id_goes_to_the_daemon() {
        assert_eq!(
            verdicts(Lang::Rust, "// Q712: kept\n"),
            [Verdict::HumanAsked(712)]
        );
        assert_eq!(
            verdicts(Lang::Rust, "// see Q712\n// Q712a\n"),
            [Verdict::Refused; 2]
        );
    }

    #[test]
    fn strip_keeps_the_code_and_the_allowed_comments() {
        let strip = |lang, text: &str| without(text, &comments(lang, text).unwrap());
        assert_eq!(
            strip(
                Lang::Rust,
                "// a\nfn a() {} // b\n\n// c\n\nfn b(/* d */x: u8) {}\n"
            ),
            "fn a() {}\n\nfn b(x: u8) {}\n"
        );
        assert_eq!(
            strip(
                Lang::Tsx,
                "const a = <p>{/* b */}c</p>;\nlet d = 1 /* e */ + 2;\n"
            ),
            "const a = <p>c</p>;\nlet d = 1 + 2;\n"
        );
        assert_eq!(
            strip(Lang::Bash, "#!/bin/sh\n# a\necho b # c\n"),
            "#!/bin/sh\necho b\n"
        );
    }

    #[test]
    fn every_kind_has_a_parser_that_reads_its_comments() {
        for (lang, text) in [
            (Lang::PowerShell, "# a\n$b = 1\n<# c #>\n"),
            (Lang::Kotlin, "// a\nval b = \"// c\" /* d */\n"),
            (
                Lang::Xml,
                "<?xml version=\"1.0\"?>\n<!-- a -->\n<b><!-- c --></b>\n",
            ),
            (Lang::Css, "/* a */\nb { color: red; } /* c */\n"),
            (
                Lang::Html,
                "<style>/* a */</style><script>// b\n</script>\n",
            ),
            (Lang::HashLines, "# a\nb#c\n#d\n"),
            (Lang::Properties, "  ! a\nb=#c\n# d\n"),
        ] {
            assert_eq!(verdicts(lang, text), [Verdict::Refused; 2], "{lang:?}");
        }
    }
}
