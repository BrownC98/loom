//! `loom show`와 `--doc`(`loom ask`, `loom done`): user-doc 스킬의 문서를 점검하고 문서 ID를 붙인다. 사람의 브라우저는 열지 않는다.
//! 사람은 질문 줄의 문서 링크로 연다(2026-10-21 사람 지시 "목업이나 보고서 승인도 질문탭에 링크로 달아두는 식으루 구현하라, 직접 브라우저에 띄우지 말고").
//! bash가 없는 Windows 노드에서도 같은 일을 하도록 Rust에 둔다. 렌더 그림은 `show.sh`가 한다.
//! 근거: skills/user-doc/SKILL.md 「보이기」.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use ts_rs::TS;

use crate::graph::WaitDoc;
use crate::link_text::DOC_LINK;

/// 문서 머리 줄과 창 제목에 붙는 ID의 틀. 다시 열면 이 틀을 찾아 바꾼다.
const ID_OPEN: &str = r#"<span class="doc-id">"#;
const WHO: &str = r#"<div class="who">"#;

pub fn show(path: &Path) -> Result<(), String> {
    let doc = attach(path)?;
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
pub fn attach(path: &Path) -> Result<WaitDoc, String> {
    attach_in(&Store::here()?, path)
}

fn attach_in(store: &Store, path: &Path) -> Result<WaitDoc, String> {
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
        let id = store.doc_id(&path)?;
        store.keep(&id, &path, &text)?;
        let title = text_title(&text, &path);
        return Ok(WaitDoc { id, title });
    }
    check(&text)?;
    if !mockup(&path) {
        check_report(&text)?;
    }
    let id = store.doc_id(&path)?;
    let title = bare_title(title(&text).unwrap_or_default()).to_owned();
    let stamped = stamp(&text, &id);
    fs::write(&path, &stamped).map_err(|e| e.to_string())?;
    store.keep(&id, &path, &stamped)?;
    Ok(WaitDoc { id, title })
}

/// 문서 ID의 파일. 원본이 있으면 원본이다(고친 새 판이 보인다). 원본이 지워졌으면(병합 뒤 지운 워크트리의 목업) 붙일 때 남긴 사본(`keep`)이다.
/// 대기가 닫힌 뒤에도 연다. 근거: 2026-10-23 사람 "왜 내가 목업을 못보나", "클로드 아티팩트처럼 상시 볼 수 있게 하면 안됨/".
/// 돌려준 파일과 같은 폴더의 파일이 문서 속 상대 주소(옆 그림)다. 사본 폴더에는 그 문서 하나만 있다.
pub fn doc_file(id: &str) -> Result<DocFile, String> {
    Store::here()?.doc_file(id)
}

/// `doc_file`이 찾은 문서.
#[derive(PartialEq, Debug)]
pub struct DocFile {
    pub path: PathBuf,
    /// 문서 속 상대 주소(옆 그림)로 읽어도 되는 폴더. 목업(`docs/mockups` 아래)과 사본은 그 파일의 폴더다.
    /// 원본 보고서는 없다. 보고서의 폴더(`~/.cache/loom-briefs` 등)에는 다른 지시와 비밀 파일이 있다(2026-10-23 검토 MAJOR).
    pub folder: Option<PathBuf>,
}

/// [문서] 탭의 한 줄(목업 docs/mockups/doc-reopen.html M13 ③). 사람은 닫힌 질문의 문서도 이 목록에서 다시 연다.
#[derive(Serialize, TS, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DocRow {
    pub id: String,
    pub title: String,
    pub kind: DocGroup,
    /// 목업은 `docs/mockups`를 가진 폴더(워크트리·저장소) 이름, 보고서는 그 파일의 폴더 이름.
    pub folder: String,
    /// 마지막으로 붙인 때(사본의 수정 시각, 유닉스 초). 이 판 전에 붙인 문서는 사본이 없어 없다.
    #[ts(type = "number | null")]
    pub attached_at: Option<i64>,
    /// 원본 파일이 있다.
    pub source: bool,
    /// 붙일 때 남긴 사본이 있다.
    pub snapshot: bool,
}

#[derive(Serialize, TS, PartialEq, Eq, Clone, Copy, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DocGroup {
    Mockup,
    Report,
}

/// 문서 ID 표의 모든 문서. 최근에 붙인 문서가 위다. 붙인 때를 모르는 문서는 그 뒤에 ID 번호가 큰 것부터 둔다.
pub fn docs() -> Result<Vec<DocRow>, String> {
    Ok(Store::here()?.docs())
}

/// 문서 ID 표와 사본 폴더의 자리. 시험은 임시 폴더를 쓴다.
struct Store {
    /// `$HOME/.cache/user-doc/ids.tsv`: 경로와 ID 두 열. 줄을 더하기만 한다.
    table: PathBuf,
    /// `<데이터 폴더>/docs`. 데몬 DB(`db::data_dir`) 옆이다.
    copies: PathBuf,
}

impl Store {
    fn here() -> Result<Self, String> {
        let home = std::env::home_dir().ok_or("홈 폴더를 찾지 못했다")?;
        let data = crate::db::data_dir().map_err(|e| e.to_string())?;
        Ok(Store {
            table: home.join(".cache").join("user-doc").join("ids.tsv"),
            copies: data.join("docs"),
        })
    }

    fn rows(&self) -> String {
        fs::read_to_string(&self.table).unwrap_or_default()
    }

    fn doc_file(&self, id: &str) -> Result<DocFile, String> {
        let source = known_id(id)
            .then(|| self.rows())
            .and_then(|rows| {
                table_rows(&rows)
                    .find(|(_, row_id)| *row_id == id)
                    .map(|(path, _)| PathBuf::from(path))
            })
            .filter(|path| doc_kind(path).is_some())
            .ok_or_else(|| format!("문서 {id}이(가) 없습니다."))?;
        if source.is_file() {
            let folder = mockup(&source)
                .then(|| source.parent().map(Path::to_path_buf))
                .flatten();
            return Ok(DocFile {
                path: source,
                folder,
            });
        }
        let copy = self
            .snapshot(id, &source)
            .filter(|copy| copy.is_file())
            .ok_or_else(|| format!("문서 파일이 없습니다. {id}의 원본은 지워졌고, 이 판 전에 붙인 문서라 저장한 판도 없습니다."))?;
        Ok(DocFile {
            folder: copy.parent().map(Path::to_path_buf),
            path: copy,
        })
    }

    fn docs(&self) -> Vec<DocRow> {
        let rows = self.rows();
        let mut seen = std::collections::HashSet::new();
        // 잠금 전에 겹친 ID는 첫 줄만 둔다. `doc_file`도 첫 줄을 연다.
        let mut docs: Vec<DocRow> = table_rows(&rows)
            .filter(|(_, id)| seen.insert(*id))
            .map(|(path, id)| self.row(id, Path::new(path)))
            .collect();
        docs.sort_by_key(|d| std::cmp::Reverse((d.attached_at, number(&d.id))));
        docs
    }

    fn row(&self, id: &str, source: &Path) -> DocRow {
        let copy = self.snapshot(id, source).filter(|c| c.is_file());
        let live = source.is_file();
        let shown = if live { Some(source) } else { copy.as_deref() };
        let text = shown.and_then(|p| fs::read_to_string(p).ok());
        let title = match (doc_kind(source), text) {
            (Some(DocKind::Html), Some(text)) => title(&text).map(|t| bare_title(t).to_owned()),
            (Some(DocKind::Text), Some(text)) => Some(text_title(&text, source)),
            _ => None,
        };
        let file_name = || {
            source
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let attached_at = copy
            .as_deref()
            .and_then(|c| fs::metadata(c).ok()?.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);
        DocRow {
            id: id.to_owned(),
            title: title.filter(|t| !t.is_empty()).unwrap_or_else(file_name),
            kind: if id.starts_with('M') {
                DocGroup::Mockup
            } else {
                DocGroup::Report
            },
            folder: folder(source),
            attached_at,
            source: live,
            snapshot: copy.is_some(),
        }
    }

    /// 원본이 지워져도 `doc_file`이 내도록 붙인 판을 사본으로 남긴다. 붙일 때마다 덮어쓴다.
    fn keep(&self, id: &str, source: &Path, text: &str) -> Result<(), String> {
        let copy = self
            .snapshot(id, source)
            .ok_or_else(|| format!("{}: 파일 이름이 없다", source.display()))?;
        let dir = copy.parent().unwrap_or(&self.copies);
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        // 새 판을 옆 임시 파일에 다 쓴 뒤 바꿔 끼운다. 쓰다 실패하면 옛 사본이 그대로 남는다(2026-10-23 검토).
        // 이름은 붙일 때마다 다르다. 같은 문서를 동시에 붙여도 서로의 임시 파일을 덮지 않는다.
        let next = dir.join(format!(".{}.tmp", crate::devices::random_token()));
        let written = fs::write(&next, text).and_then(|()| fs::rename(&next, &copy));
        if written.is_err() {
            let _ = fs::remove_file(&next);
        }
        written.map_err(|e| format!("{}: {e}", copy.display()))
    }

    /// `<데이터 폴더>/docs/<ID>/<원본 파일 이름>`. 이름이 같아 형식(`doc_kind`)도 같다.
    fn snapshot(&self, id: &str, source: &Path) -> Option<PathBuf> {
        Some(self.copies.join(id).join(source.file_name()?))
    }

    /// 사람이 어느 문서에 답하는지 짧게 말하게 한다. 같은 경로는 판이 바뀌어도 같은 ID다.
    /// 목업(`docs/mockups/` 아래)은 M, 보고서는 R에 번호를 붙인다. 표는 경로와 ID 두 열이다.
    fn doc_id(&self, doc: &Path) -> Result<String, String> {
        let table = &self.table;
        if let Some(dir) = table.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        // 읽기·찾기·더하기를 한 잠금 안에서 한다. 잠그지 않으면 동시에 붙인 문서가 같은 ID를 받고 줄이 섞였다(2026-10-23 검토: `loom show` 48개 동시 실행에 ID 38개, 깨진 줄 8개).
        // 잠금은 옆 파일에 건다. Windows 잠금은 강제라서 표 자체를 잠그면 그동안 `doc_file`이 표를 읽지 못한다.
        let lock = table.with_extension("tsv.lock");
        let guard = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock)
            .map_err(|e| format!("{}: {e}", lock.display()))?;
        guard
            .lock()
            .map_err(|e| format!("{}: {e}", lock.display()))?;
        let rows = self.rows();
        let key = doc.to_string_lossy();
        let (id, new) = lookup(&rows, &key, mockup(doc));
        if new {
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(table)
                .map_err(|e| format!("{}: {e}", table.display()))?;
            // 한 줄을 쓰기 한 번에 쓴다.
            file.write_all(format!("{key}\t{id}\n").as_bytes())
                .map_err(|e| e.to_string())?;
        }
        Ok(id)
    }
}

/// 목업은 `docs/mockups` 위 폴더, 보고서는 바로 위 폴더의 이름.
fn folder(source: &Path) -> String {
    let dir = source
        .ancestors()
        .find(|dir| dir.ends_with("docs/mockups"))
        .and_then(|mockups| mockups.parent()?.parent())
        .or_else(|| source.parent());
    dir.and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `M3`, `R12`의 꼴.
fn known_id(id: &str) -> bool {
    id.strip_prefix(['M', 'R'])
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn number(id: &str) -> u32 {
    id[1..].parse().unwrap_or(0)
}

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
        match crate::link_text::parse_link(link) {
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

fn mockup(doc: &Path) -> bool {
    doc.ancestors().any(|dir| dir.ends_with("docs/mockups"))
}

/// 표에 있는 ID를 돌려준다. 없으면 같은 종류의 다음 번호를 만들고 `true`를 같이 준다.
/// 새 번호는 그 종류의 가장 큰 번호 다음이다. 잠금 전에 깨진 표(섞인 줄, 겹친 ID)에서도 겹치지 않는다.
fn lookup(rows: &str, key: &str, mockup: bool) -> (String, bool) {
    let rows = table_rows(rows);
    if let Some((_, id)) = rows.clone().find(|(path, _)| *path == key) {
        return (id.to_string(), false);
    }
    let kind = if mockup { 'M' } else { 'R' };
    let last = rows
        .filter(|(_, id)| id.starts_with(kind))
        .map(|(_, id)| number(id))
        .max()
        .unwrap_or(0);
    (format!("{kind}{}", last + 1), true)
}

/// 표의 (경로, ID) 줄. ID 꼴이 아닌 줄(잠금 전에 섞인 줄)은 건너뛴다.
fn table_rows(rows: &str) -> impl Iterator<Item = (&str, &str)> + Clone {
    rows.lines()
        .filter_map(|row| row.split_once('\t'))
        .filter(|(path, id)| !path.is_empty() && known_id(id))
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
        // 잠금 전에 깨진 표: 섞인 줄은 건너뛰고, 새 번호는 가장 큰 번호 다음이다.
        let broken = "/a\tR1\n/b\tR3/c\tR2\n/d\tR3\n/e\tR3\n\tR9\n";
        assert_eq!(lookup(broken, "/x", false), ("R4".into(), true));
        assert_eq!(lookup(broken, "/d", false), ("R3".into(), false));
    }

    /// 여러 스레드가 동시에 붙여도 ID가 겹치지 않고 줄이 깨지지 않는다.
    #[test]
    fn concurrent_registration_gives_unique_ids_and_whole_rows() {
        let home = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(Store {
            table: home.path().join("ids.tsv"),
            copies: home.path().join("copies"),
        });
        let start = std::sync::Arc::new(std::sync::Barrier::new(16));
        let threads: Vec<_> = (0..16)
            .map(|t| {
                let (store, start) = (store.clone(), start.clone());
                std::thread::spawn(move || {
                    start.wait();
                    (0..30)
                        .map(|i| {
                            store
                                .doc_id(Path::new(&format!("/r/report-{t}-{i}.md")))
                                .unwrap()
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut ids: Vec<String> = threads
            .into_iter()
            .flat_map(|t| t.join().unwrap())
            .collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 480);
        let rows = fs::read_to_string(&store.table).unwrap();
        assert_eq!(rows.lines().count(), 480);
        assert_eq!(table_rows(&rows).count(), 480);
    }

    #[test]
    fn a_doc_row_names_its_folder() {
        assert_eq!(folder(Path::new("/w/폰연결/docs/mockups/m.html")), "폰연결");
        assert_eq!(
            folder(Path::new("/w/폰연결/docs/mockups/old/m.html")),
            "폰연결"
        );
        assert_eq!(
            folder(Path::new("/h/.cache/loom-briefs/r.md")),
            "loom-briefs"
        );
    }

    #[test]
    fn doc_ids_have_one_letter_and_a_number() {
        for id in ["M1", "R42"] {
            assert!(known_id(id), "{id}");
        }
        for id in ["", "M", "X1", "M1a", "../R1", "R-1"] {
            assert!(!known_id(id), "{id}");
        }
    }

    #[test]
    fn docs_outlive_their_source_and_list_newest_first() {
        let home = tempfile::tempdir().unwrap();
        let store = Store {
            table: home.path().join("ids.tsv"),
            copies: home.path().join("copies"),
        };
        let repo = home.path().join("작업");
        fs::create_dir_all(repo.join("docs/mockups")).unwrap();
        let mock = repo.join("docs/mockups/m.html");
        fs::write(&mock, DOC).unwrap();
        let report = repo.join("r.md");
        fs::write(&report, "# 감사\n본문\n").unwrap();
        fs::write(repo.join("old.md"), "").unwrap();
        // 이 판 전에 붙인 문서: 표에만 있고 사본이 없다.
        let old = store
            .doc_id(&real_path(&repo.join("old.md")).unwrap())
            .unwrap();
        fs::remove_file(repo.join("old.md")).unwrap();
        let m = attach_in(&store, &mock).unwrap().id;
        let r = attach_in(&store, &report).unwrap().id;
        assert_eq!((old.as_str(), m.as_str(), r.as_str()), ("R1", "M1", "R2"));

        // 원본이 있으면 원본을 낸다(새 판이 보인다). 원본 보고서는 옆 파일을 내지 않는다. 원본 목업은 그 폴더다.
        fs::write(&report, "# 감사 2\n").unwrap();
        assert_eq!(
            store.doc_file(&r),
            Ok(DocFile {
                path: real_path(&report).unwrap(),
                folder: None
            })
        );
        let real_mock = real_path(&mock).unwrap();
        assert_eq!(
            store.doc_file(&m).unwrap().folder.as_deref(),
            real_mock.parent()
        );
        // 원본이 지워지면 붙인 판의 사본을 낸다. 사본 폴더에는 그 문서 하나만 있다.
        fs::remove_file(&mock).unwrap();
        let DocFile { path: copy, folder } = store.doc_file(&m).unwrap();
        assert_eq!(folder.as_deref(), copy.parent());
        // 사본은 임시 파일에 쓴 뒤 바꿔 끼운다. 폴더에는 그 문서 하나만 남는다.
        assert_eq!(fs::read_dir(copy.parent().unwrap()).unwrap().count(), 1);
        assert!(
            copy.starts_with(home.path().join("copies").join("M1")),
            "{}",
            copy.display()
        );
        assert!(
            fs::read_to_string(&copy)
                .unwrap()
                .contains("<title>M1 문서</title>")
        );
        assert!(
            store
                .doc_file(&old)
                .unwrap_err()
                .contains("문서 파일이 없습니다")
        );
        assert!(store.doc_file("R99").is_err());
        assert!(store.doc_file("../R2").is_err());

        let rows = store.docs();
        let ids: Vec<&str> = rows.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids[2], "R1", "사본이 없으면 뒤로 간다: {ids:?}");
        let mock_row = rows.iter().find(|d| d.id == m).unwrap();
        assert_eq!(
            (
                mock_row.title.as_str(),
                mock_row.kind,
                mock_row.folder.as_str()
            ),
            ("문서", DocGroup::Mockup, "작업")
        );
        assert!(!mock_row.source && mock_row.snapshot && mock_row.attached_at.is_some());
        let report_row = rows.iter().find(|d| d.id == r).unwrap();
        assert_eq!(report_row.title, "감사 2");
        assert!(report_row.source && report_row.snapshot);
        let old_row = &rows[2];
        assert_eq!(
            (old_row.title.as_str(), old_row.source, old_row.snapshot),
            ("old.md", false, false)
        );
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
