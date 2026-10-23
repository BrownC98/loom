//! `loom show`와 `--doc`(`loom ask`, `loom done`): user-doc 스킬의 문서를 점검하고 문서 ID를 붙인다. 사람의 브라우저는 열지 않는다.
//! 사람은 질문 줄의 문서 링크로 연다(2026-10-21 사람 지시 "목업이나 보고서 승인도 질문탭에 링크로 달아두는 식으루 구현하라, 직접 브라우저에 띄우지 말고").
//! bash가 없는 Windows 노드에서도 같은 일을 하도록 Rust에 둔다. 렌더 그림은 `show.sh`가 한다.
//! 근거: skills/user-doc/SKILL.md 「보이기」.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::graph::{Attached, WaitDoc};

/// 문서 머리 줄과 창 제목에 붙는 ID의 틀. 다시 열면 이 틀을 찾아 바꾼다.
const ID_OPEN: &str = r#"<span class="doc-id">"#;
const WHO: &str = r#"<div class="who">"#;

pub fn show(path: &Path) -> Result<(), String> {
    let Attached { doc, path } = attach(path)?;
    println!("show: 문서 ID {} ({})", doc.id, doc.title);
    println!(
        "show: 사람에게 보이려면 질문에 붙인다: loom ask \"<질문>\" --doc {}",
        path.display()
    );
    Ok(())
}

/// `--doc`이 받는 문서의 갈래. 붙이기(`attach`)와 데몬의 내기(`daemon::serve_file`)가 이 한 목록을 쓴다.
/// 그 밖의 파일(확장자 없는 README, `.csv`, `.svg` 등)은 받지 않는다. 받으면 브라우저가 내려받거나 스크립트를 돌린다(2026-10-23 검토 BLOCK).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DocKind {
    /// user-doc 문서. 점검하고 ID를 붙인다.
    Html,
    /// 글 파일(`.md` 보고서 등). 점검하지 않고 고치지 않는다. 데몬은 `text/plain; charset=utf-8`로 낸다.
    Text,
}

const HTML_EXTENSIONS: [&str; 2] = ["html", "htm"];
const TEXT_EXTENSIONS: [&str; 4] = ["md", "markdown", "txt", "log"];

pub fn doc_kind(path: &Path) -> Option<DocKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if HTML_EXTENSIONS.contains(&ext.as_str()) {
        Some(DocKind::Html)
    } else if TEXT_EXTENSIONS.contains(&ext.as_str()) {
        Some(DocKind::Text)
    } else {
        None
    }
}

/// 문서를 점검하고 ID를 붙인다. 같은 문서는 판이 바뀌어도 같은 ID다.
/// 근거: 총괄이 `.md` 감사 보고서를 붙이지 못해 경로를 글로 적었고, 사람은 열지 못했다(2026-10-23 사람 "링크로 제공이 안됨").
pub fn attach(path: &Path) -> Result<Attached, String> {
    let path = real_path(path)?;
    // 총괄이 그림(PNG)을 붙여 「stream did not contain valid UTF-8」만 받았다(2026-10-22).
    let refused = || {
        format!(
            "{}: --doc takes a user-doc HTML file (.{}) or a text file (.{}); put images inside an HTML file.",
            path.display(),
            HTML_EXTENSIONS.join(", ."),
            TEXT_EXTENSIONS.join(", .")
        )
    };
    let kind = doc_kind(&path).ok_or_else(refused)?;
    let text = fs::read_to_string(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::InvalidData => refused(),
        _ => format!("{}: {e}", path.display()),
    })?;
    if kind == DocKind::Text {
        let title = text_title(&text, &path);
        return Ok(Attached {
            doc: WaitDoc {
                id: doc_id(&path)?,
                title,
            },
            path,
        });
    }
    check(&text)?;
    if !mockup(&path) {
        check_report(&text)?;
    }
    let id = doc_id(&path)?;
    let title = bare_title(title(&text).unwrap_or_default()).to_owned();
    fs::write(&path, stamp(&text, &id)).map_err(|e| e.to_string())?;
    Ok(Attached {
        doc: WaitDoc { id, title },
        path,
    })
}

/// 본문 속 문서 링크의 주소 표지. 화면(`web/src/links.ts` `DOC_LINK`)이 이 자리에 붙은 문서의 링크를 그린다.
/// 근거: 2026-10-23 사람 지시 "질문 목업 링크도 아래 완료의 링크처럼 본문에 하이퍼 링크 달리는 방식으로 만들어줘".
pub const DOC_LINK: &str = "loom:doc";

/// 글 속 markdown 링크 `[이름](주소)` 가운데 주소가 `doc` 파일인 것의 주소를 `DOC_LINK`로 바꾼다.
/// 주소는 `file://` 주소(퍼센트 인코딩 포함)나 경로이고, WSL 노드의 Linux 경로도 받는다. 파일 경로는 데몬만 알아야 하므로 글에 남기지 않는다.
pub fn inline_link(text: &str, doc: &Path) -> String {
    let Ok(doc) = real_path(doc) else {
        return text.to_owned();
    };
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let (before, link) = rest.split_at(open);
        out.push_str(before);
        let parsed = link[1..].split_once("](").and_then(|(label, after)| {
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
        });
        match parsed {
            Some((label, target, len))
                if link_path(target).is_some_and(|p| real_path(&p).is_ok_and(|p| p == doc)) =>
            {
                out.push_str(&format!("[{label}]({DOC_LINK})"));
                rest = &link[len..];
            }
            _ => {
                out.push('[');
                rest = &link[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// 링크 주소가 가리키는 파일 경로. `file:///C:/…`는 `C:/…`, `file://host/…`는 UNC `//host/…`다.
fn link_path(target: &str) -> Option<PathBuf> {
    let path = match target.strip_prefix("file://") {
        Some(rest) => {
            let rest = percent_encoding::percent_decode_str(rest)
                .decode_utf8()
                .ok()?;
            match rest.strip_prefix('/') {
                Some(local) if local.as_bytes().get(1) == Some(&b':') => local.to_owned(),
                Some(_) => rest.into_owned(),
                None => format!("//{rest}"),
            }
        }
        None if target.contains("://") => return None,
        None => target.to_owned(),
    };
    Some(crate::wsl::from_caller(PathBuf::from(path)))
}

/// 글 문서의 제목: 첫 markdown 제목 줄(`# …`), 없으면 파일 이름.
fn text_title(text: &str, path: &Path) -> String {
    let heading = text
        .lines()
        .find_map(|l| l.strip_prefix("# "))
        .map(str::trim)
        .filter(|t| !t.is_empty());
    match heading {
        Some(t) => t.to_owned(),
        None => path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}

/// 같은 문서가 언제나 같은 표 열쇠를 갖게 링크를 푼다. Windows의 `\\?\C:\…`는 앱이 못 읽으므로 `C:\…`로 둔다.
fn real_path(path: &Path) -> Result<PathBuf, String> {
    let real = fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(without_verbatim(real))
}

fn without_verbatim(real: PathBuf) -> PathBuf {
    let drive = real
        .to_string_lossy()
        .strip_prefix(r"\\?\")
        .filter(|rest| !rest.starts_with(r"UNC\"))
        .map(PathBuf::from);
    drive.unwrap_or(real)
}

/// style.html에서 시작한 문서인지 본다.
fn check(html: &str) -> Result<(), String> {
    if title(html).is_none_or(str::is_empty) {
        return Err("<title>이 없다".into());
    }
    let marks = [
        (
            r#"style="color-scheme: light""#,
            r#"<html style="color-scheme: light">가 없다(기본은 라이트)"#,
        ),
        (
            r#"class="theme-toggle""#,
            "라이트/다크 전환 버튼(.theme-toggle)이 없다",
        ),
        ("응답할 곳", "머리 줄 「응답할 곳」이 없다"),
        (WHO, r#"머리 줄(<div class="who">)이 없다"#),
    ];
    match marks.iter().find(|(mark, _)| !html.contains(mark)) {
        Some((_, missing)) => Err((*missing).into()),
        None => Ok(()),
    }
}

/// 보고서는 그림으로 보인다(2026-10-22 사람 지시 "다이어그램 등으로 시각화 해서 이해하기 쉽게 작성해, 앞으로 모든 보고서에 적용되는 규칙으로 설정").
/// 목업은 화면 그림이 본문이라 보지 않는다.
fn check_report(html: &str) -> Result<(), String> {
    if html.contains("<svg") || html.contains("<img") {
        Ok(())
    } else {
        Err("보고서에 그림(<svg>·<img>)이 없다. 결론과 구조를 다이어그램으로 보인다".into())
    }
}

fn title(html: &str) -> Option<&str> {
    let start = html.find("<title>")? + "<title>".len();
    let len = html[start..].find('<').unwrap_or(html.len() - start);
    Some(&html[start..start + len])
}

/// 앞에 붙은 문서 ID(`M3 `, `R12 `)를 모두 뗀 제목. 다른 HOME의 ID 표에서 붙은 ID도 뗀다. 화면은 ID를 따로 보인다.
fn bare_title(title: &str) -> &str {
    let mut bare = title;
    while let Some(rest) = bare.strip_prefix(['M', 'R']) {
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        match rest[digits..].strip_prefix(' ') {
            Some(next) if digits > 0 => bare = next,
            _ => break,
        }
    }
    bare
}

/// 사람이 어느 문서에 답하는지 짧게 말하게 한다. 같은 경로는 판이 바뀌어도 같은 ID다.
/// 목업(`docs/mockups/` 아래)은 M, 보고서는 R에 번호를 붙인다. 표는 경로와 ID 두 열이다.
fn doc_id(doc: &Path) -> Result<String, String> {
    let home = std::env::home_dir().ok_or("홈 폴더를 찾지 못했다")?;
    let table = home.join(".cache").join("user-doc").join("ids.tsv");
    fs::create_dir_all(table.parent().unwrap_or(&home)).map_err(|e| e.to_string())?;
    let rows = fs::read_to_string(&table).unwrap_or_default();
    let key = doc.to_string_lossy();
    let (id, new) = lookup(&rows, &key, mockup(doc));
    if new {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&table)
            .map_err(|e| format!("{}: {e}", table.display()))?;
        writeln!(file, "{key}\t{id}").map_err(|e| e.to_string())?;
    }
    Ok(id)
}

fn mockup(doc: &Path) -> bool {
    doc.ancestors().any(|dir| dir.ends_with("docs/mockups"))
}

/// 표에 있는 ID를 돌려준다. 없으면 같은 종류의 다음 번호를 만들고 `true`를 같이 준다.
fn lookup(rows: &str, key: &str, mockup: bool) -> (String, bool) {
    let rows = rows.lines().filter_map(|row| row.split_once('\t'));
    if let Some((_, id)) = rows.clone().find(|(path, _)| *path == key) {
        return (id.to_string(), false);
    }
    let kind = if mockup { 'M' } else { 'R' };
    let used = rows.filter(|(_, id)| id.starts_with(kind)).count();
    (format!("{kind}{}", used + 1), true)
}

/// 머리 줄 맨 앞과 `<title>` 앞에 ID를 붙인다. 이미 붙어 있으면 바꾸고, 제목에는 한 번만 붙인다.
fn stamp(html: &str, id: &str) -> String {
    let tag = format!("{ID_OPEN}문서 ID: <b>{id}</b></span>");
    let mut out = match html.find(ID_OPEN) {
        Some(start) => {
            let end = html[start..]
                .find("</span>")
                .map_or(html.len(), |i| start + i + "</span>".len());
            format!("{}{tag}{}", &html[..start], &html[end..])
        }
        None => html.replacen(WHO, &format!("{WHO}{tag}"), 1),
    };
    if let Some(old) = title(&out).map(str::to_owned) {
        let new = format!("<title>{id} {}", bare_title(&old));
        out = out.replacen(&format!("<title>{old}"), &new, 1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"<html style="color-scheme: light"><head><title>문서</title></head>
<body><button class="theme-toggle"></button><div class="who">응답할 곳: 총괄</div></body></html>"#;

    #[test]
    fn stamp_adds_id_once() {
        let once = stamp(DOC, "M3");
        assert!(once.contains("<title>M3 문서</title>"));
        assert!(once.contains(
            r#"<div class="who"><span class="doc-id">문서 ID: <b>M3</b></span>응답할 곳"#
        ));
        assert_eq!(stamp(&once, "M3"), once);
        // 다른 ID 표에서 붙은 ID는 바꾼다.
        assert!(stamp(&once, "M7").contains("<title>M7 문서</title>"));
        assert!(check(&once).is_ok());
    }

    #[test]
    fn an_image_is_refused_with_the_file_to_attach() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("mockup.png");
        fs::write(&png, [0x89, b'P', b'N', b'G', 0xff, 0xfe]).unwrap();
        let refused = attach(&png).err().unwrap();
        assert!(
            refused.contains("put images inside an HTML file"),
            "{refused}"
        );
    }

    #[test]
    fn a_link_to_the_doc_becomes_the_doc_link() {
        let dir = tempfile::tempdir().unwrap();
        let doc = dir.path().join("목업 a.html");
        let other = dir.path().join("b.html");
        fs::write(&doc, "").unwrap();
        fs::write(&other, "").unwrap();
        // Windows 경로는 `file:///C:/…`다.
        let slashed = doc.display().to_string().replace('\\', "/");
        let url = format!(
            "file://{}{}",
            if slashed.starts_with('/') { "" } else { "/" },
            percent_encoding::utf8_percent_encode(&slashed, percent_encoding::NON_ALPHANUMERIC)
                .to_string()
                .replace("%2F", "/")
                .replace("%3A", ":")
        );
        let other = other.display().to_string();
        let text = format!("[문헌 도구]({url}) 승인? [다른]({other}) [x] (y) [웹](https://a.b/c)");
        assert_eq!(
            inline_link(&text, &doc),
            format!("[문헌 도구]({DOC_LINK}) 승인? [다른]({other}) [x] (y) [웹](https://a.b/c)")
        );
        assert_eq!(inline_link("승인? (A / B)", &doc), "승인? (A / B)");
        let path = doc.display();
        assert_eq!(
            inline_link(&format!("[목업](<{path}>) 승인? [x](<{path}>"), &doc),
            format!("[목업]({DOC_LINK}) 승인? [x](<{path}>")
        );
        let plain = dir.path().join("plain.md");
        fs::write(&plain, "").unwrap();
        let plain_path = plain.display();
        assert_eq!(
            inline_link(&format!("[보고서](<{plain_path}>)."), &plain),
            format!("[보고서]({DOC_LINK}).")
        );
    }

    #[test]
    fn the_web_knows_the_doc_link() {
        let links = include_str!("../web/src/links.ts");
        assert!(links.contains(&format!("DOC_LINK = '{DOC_LINK}'")));
    }

    #[test]
    fn a_text_doc_takes_its_first_heading_or_file_name() {
        let md = Path::new("/r/audit.md");
        assert_eq!(text_title("앞말\n# 알림 감사 \n본문\n", md), "알림 감사");
        assert_eq!(text_title("본문만\n#붙은 줄", md), "audit.md");
    }

    #[test]
    fn only_listed_extensions_are_docs() {
        assert_eq!(doc_kind(Path::new("/r/a.HTML")), Some(DocKind::Html));
        assert_eq!(doc_kind(Path::new("/r/a.htm")), Some(DocKind::Html));
        assert_eq!(doc_kind(Path::new("/r/audit.MD")), Some(DocKind::Text));
        assert_eq!(doc_kind(Path::new("/r/run.log")), Some(DocKind::Text));
        assert_eq!(doc_kind(Path::new("/r/README")), None);
        assert_eq!(doc_kind(Path::new("/r/rows.csv")), None);
        assert_eq!(doc_kind(Path::new("/r/a.svg")), None);
    }

    #[test]
    fn other_text_files_are_refused_with_the_list() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["README", "rows.csv", "a.svg"] {
            let file = dir.path().join(name);
            fs::write(&file, "<svg><script>x</script></svg>").unwrap();
            let refused = attach(&file).err().unwrap();
            assert!(refused.contains(".md, .markdown, .txt, .log"), "{refused}");
        }
    }

    #[test]
    fn bare_title_drops_only_a_leading_doc_id() {
        assert_eq!(bare_title("M25 질문 목업"), "질문 목업");
        assert_eq!(bare_title("R3 보고서"), "보고서");
        assert_eq!(bare_title("M1 M25 질문 목업"), "질문 목업");
        assert_eq!(bare_title("Mockup of M25"), "Mockup of M25");
        assert_eq!(bare_title("M 목업"), "M 목업");
        assert_eq!(bare_title("목업"), "목업");
    }

    #[test]
    fn check_names_the_missing_mark() {
        let err = check(&DOC.replace("theme-toggle", "x")).unwrap_err();
        assert!(err.contains(".theme-toggle"));
        assert!(check(&DOC.replace("<title>문서", "<title>")).is_err());
    }

    #[test]
    fn reports_need_a_picture() {
        assert!(check_report(DOC).is_err());
        assert!(check_report(&DOC.replace("</body>", "<svg></svg></body>")).is_ok());
        assert!(check_report(&DOC.replace("</body>", r#"<img src="a.png"></body>"#)).is_ok());
    }

    #[test]
    fn lookup_keeps_ids_and_counts_per_kind() {
        let rows = "/a/x.html\tM1\n/b/y.html\tR1\n/c/z.html\tM2\n";
        assert_eq!(lookup(rows, "/b/y.html", true), ("R1".into(), false));
        assert_eq!(lookup(rows, "/d/n.html", true), ("M3".into(), true));
        assert_eq!(lookup(rows, "/d/n.html", false), ("R2".into(), true));
    }

    #[test]
    fn mockups_are_under_docs_mockups() {
        assert!(mockup(Path::new("/r/docs/mockups/a.html")));
        assert!(mockup(Path::new("/r/docs/mockups/old/a.html")));
        assert!(!mockup(Path::new("/r/docs/a.html")));
    }

    #[test]
    fn windows_paths_lose_only_the_drive_verbatim_prefix() {
        let plain = |p: &str| without_verbatim(PathBuf::from(p));
        assert_eq!(
            plain(r"\\?\C:\Users\me\a.html"),
            PathBuf::from(r"C:\Users\me\a.html")
        );
        let unc = r"\\?\UNC\wsl.localhost\Ubuntu\a.html";
        assert_eq!(plain(unc), PathBuf::from(unc));
        assert_eq!(plain("/home/me/a.html"), PathBuf::from("/home/me/a.html"));
    }
}
