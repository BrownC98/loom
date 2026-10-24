use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use ts_rs::TS;

use crate::link_text::DOC_LINK;
use crate::protocol::{Refusal, WaitDoc};

const ID_OPEN: &str = r#"<span class="doc-id">"#;
const WHO: &str = r#"<div class="who">"#;

pub fn show(path: &Path) -> Result<(), String> {
    let store = Store::here()?;
    let doc = stamp_in(&store, path)?;
    println!("show: 문서 ID {} ({})", doc.id, doc.title);
    if bashless() {
        pass_in(&store, path)?;
        println!("show: bash 없는 Windows 노드라 이 점검으로 통과를 남긴다");
        println!(
            "show: 사람에게 보이려면 질문에 붙인다: loom ask \"<질문>\" --doc {}",
            path.display()
        );
    } else {
        println!("show: --doc 통과는 show.sh가 렌더·말풍선 점검 뒤에 남긴다");
    }
    Ok(())
}

pub fn record_checked(path: &Path) -> Result<(), String> {
    let doc = pass_in(&Store::here()?, path)?;
    println!("show: 문서 ID {} 통과", doc.id);
    println!(
        "show: 사람에게 보이려면 질문에 붙인다: loom ask \"<질문>\" --doc {}",
        path.display()
    );
    Ok(())
}

fn bashless() -> bool {
    cfg!(windows)
        && std::env::current_dir()
            .ok()
            .and_then(|dir| crate::wsl::of(&dir))
            .is_none()
}

const HTML_EXTENSIONS: [&str; 2] = ["html", "htm"];

fn html(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| HTML_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

fn stamp_in(store: &Store, path: &Path) -> Result<WaitDoc, String> {
    let (path, text) = checked(path)?;
    let id = store.doc_id(&path)?;
    let title = bare_title(title(&text).unwrap_or_default()).to_owned();
    fs::write(&path, stamp(&text, &id)).map_err(|e| e.to_string())?;
    Ok(WaitDoc { id, title })
}

fn pass_in(store: &Store, path: &Path) -> Result<WaitDoc, String> {
    let (path, text) = checked(path)?;
    let id = store.doc_id(&path)?;
    if stamp(&text, &id) != text {
        return Err(format!(
            "{}: 문서 ID가 없다. show.sh로 점검한다",
            path.display()
        ));
    }
    store.pass(&text)?;
    let title = bare_title(title(&text).unwrap_or_default()).to_owned();
    Ok(WaitDoc { id, title })
}

fn checked(path: &Path) -> Result<(PathBuf, String), String> {
    let path = real_path(path)?;
    let text = read_html(&path)?;
    check(&text)?;
    if !mockup(&path) {
        check_report(&text)?;
    }
    Ok((path, text))
}

fn read_html(path: &Path) -> Result<String, String> {
    let refused = || {
        format!(
            "{}: --doc takes a user-doc HTML file (.{}); put images inside an HTML file.",
            path.display(),
            HTML_EXTENSIONS.join(", .")
        )
    };
    if !html(path) {
        return Err(refused());
    }
    fs::read_to_string(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::InvalidData => refused(),
        _ => format!("{}: {e}", path.display()),
    })
}

pub fn attach(path: &Path) -> Result<WaitDoc, Refusal> {
    attach_in(&Store::here().map_err(Refusal::Db)?, path)
}

fn attach_in(store: &Store, path: &Path) -> Result<WaitDoc, Refusal> {
    let refused = || Refusal::UncheckedDoc(path.display().to_string());
    let path = real_path(path).map_err(|_| refused())?;
    let text = read_html(&path).map_err(|_| refused())?;
    if !store.passed(&text) {
        return Err(refused());
    }
    let id = store.doc_id(&path).map_err(Refusal::Db)?;
    if stamp(&text, &id) != text {
        return Err(refused());
    }
    store.keep(&id, &path, &text).map_err(Refusal::Db)?;
    let title = bare_title(title(&text).unwrap_or_default()).to_owned();
    Ok(WaitDoc { id, title })
}

pub fn unchecked_link(text: &str, folder: &Path) -> Option<String> {
    let store = Store::here().ok()?;
    unchecked_link_in(&store, text, folder)
}

fn unchecked_link_in(store: &Store, text: &str, folder: &Path) -> Option<String> {
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let link = &rest[open..];
        match crate::link_text::parse_link(link) {
            Some((_, target, len)) => {
                if target.starts_with("file://") {
                    let checked = link_path(target)
                        .map(|p| crate::wsl::from_node(p, folder))
                        .and_then(|p| real_path(&p).ok())
                        .and_then(|p| read_html(&p).ok())
                        .is_some_and(|text| store.passed(&text));
                    if !checked {
                        return Some(target.to_owned());
                    }
                }
                rest = &link[len..];
            }
            None => rest = &link[1..],
        }
    }
    None
}

pub fn doc_file(id: &str) -> Result<DocFile, String> {
    Store::here()?.doc_file(id)
}

#[derive(PartialEq, Debug)]
pub struct DocFile {
    pub path: PathBuf,
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

pub fn docs() -> Result<Vec<DocRow>, String> {
    Ok(Store::here()?.docs())
}

struct Store {
    table: PathBuf,
    passes: PathBuf,
    copies: PathBuf,
}

impl Store {
    fn here() -> Result<Self, String> {
        let home = std::env::home_dir().ok_or("홈 폴더를 찾지 못했다")?;
        let data = crate::db::data_dir().map_err(|e| e.to_string())?;
        let cache = home.join(".cache").join("user-doc");
        Ok(Store {
            table: cache.join("ids.tsv"),
            passes: cache.join("passed.txt"),
            copies: data.join("docs"),
        })
    }

    fn pass(&self, text: &str) -> Result<(), String> {
        if let Some(dir) = self.passes.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.passes)
            .map_err(|e| format!("{}: {e}", self.passes.display()))?;
        file.write_all(format!("{}\n", crate::skills::digest(&[text])).as_bytes())
            .map_err(|e| format!("{}: {e}", self.passes.display()))
    }

    fn passed(&self, text: &str) -> bool {
        let digest = crate::skills::digest(&[text]);
        fs::read_to_string(&self.passes).is_ok_and(|rows| rows.lines().any(|row| row == digest))
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
            .filter(|path| html(path))
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
        let mut docs: Vec<DocRow> = table_rows(&rows)
            .filter(|(path, id)| html(Path::new(path)) && seen.insert(*id))
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
        let title = text.and_then(|text| title(&text).map(|t| bare_title(t).to_owned()));
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

    fn keep(&self, id: &str, source: &Path, text: &str) -> Result<(), String> {
        let copy = self
            .snapshot(id, source)
            .ok_or_else(|| format!("{}: 파일 이름이 없다", source.display()))?;
        let dir = copy.parent().unwrap_or(&self.copies);
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let next = dir.join(format!(".{}.tmp", crate::pake::random_token()));
        let written = fs::write(&next, text).and_then(|()| fs::rename(&next, &copy));
        if written.is_err() {
            let _ = fs::remove_file(&next);
        }
        written.map_err(|e| format!("{}: {e}", copy.display()))
    }

    fn snapshot(&self, id: &str, source: &Path) -> Option<PathBuf> {
        Some(self.copies.join(id).join(source.file_name()?))
    }

    fn doc_id(&self, doc: &Path) -> Result<String, String> {
        let table = &self.table;
        if let Some(dir) = table.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
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
            file.write_all(format!("{key}\t{id}\n").as_bytes())
                .map_err(|e| e.to_string())?;
        }
        Ok(id)
    }
}

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

fn known_id(id: &str) -> bool {
    id.strip_prefix(['M', 'R'])
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn number(id: &str) -> u32 {
    id[1..].parse().unwrap_or(0)
}

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
                if link_path(target)
                    .map(crate::wsl::from_caller)
                    .is_some_and(|p| real_path(&p).is_ok_and(|p| p == doc)) =>
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

fn link_path(target: &str) -> Option<PathBuf> {
    let path = match target.strip_prefix("file://") {
        Some(rest) => {
            let rest = rest.split(['#', '?']).next().unwrap_or_default();
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
    Some(PathBuf::from(path))
}

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

fn table_rows(rows: &str) -> impl Iterator<Item = (&str, &str)> + Clone {
    rows.lines()
        .filter_map(|row| row.split_once('\t'))
        .filter(|(path, id)| !path.is_empty() && known_id(id))
}

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
pub(crate) mod tests {
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
        assert!(stamp(&once, "M7").contains("<title>M7 문서</title>"));
        assert!(check(&once).is_ok());
    }

    fn store(home: &Path) -> Store {
        Store {
            table: home.join("ids.tsv"),
            passes: home.join("passed.txt"),
            copies: home.join("copies"),
        }
    }

    #[test]
    fn an_image_is_refused_with_the_file_to_attach() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("mockup.png");
        fs::write(&png, [0x89, b'P', b'N', b'G', 0xff, 0xfe]).unwrap();
        let refused = stamp_in(&store(dir.path()), &png).err().unwrap();
        assert!(
            refused.contains("put images inside an HTML file"),
            "{refused}"
        );
    }

    #[test]
    fn a_copy_of_a_passed_doc_does_not_attach() {
        let home = tempfile::tempdir().unwrap();
        let store = store(home.path());
        let report = home.path().join("r.html");
        fs::write(&report, DOC.replace("</body>", "<svg></svg></body>")).unwrap();
        let shown = shown_in(&store, &report);
        assert_eq!(attach_in(&store, &report), Ok(shown.clone()));
        let copy = home.path().join("r-v2.html");
        fs::copy(&report, &copy).unwrap();
        assert_eq!(
            attach_in(&store, &copy),
            Err(Refusal::UncheckedDoc(copy.display().to_string()))
        );
        let again = shown_in(&store, &copy);
        assert_ne!(again.id, shown.id);
        assert_eq!(attach_in(&store, &copy), Ok(again));
    }

    #[test]
    fn a_fragment_link_points_at_its_file() {
        let path = Path::new("/tmp/r.html");
        let url = file_url(path);
        assert_eq!(link_path(&format!("{url}#part")), link_path(&url));
        assert_eq!(link_path(&format!("{url}?a=1#b")), link_path(&url));
        assert_eq!(
            link_path("file:///tmp/a%23b.html"),
            link_path("/tmp/a#b.html")
        );
    }

    fn shown_in(store: &Store, path: &Path) -> WaitDoc {
        stamp_in(store, path).unwrap();
        pass_in(store, path).unwrap()
    }

    pub(crate) fn file_url(path: &Path) -> String {
        let slashed = path.display().to_string().replace('\\', "/");
        format!(
            "file://{}{}",
            if slashed.starts_with('/') { "" } else { "/" },
            percent_encoding::utf8_percent_encode(&slashed, percent_encoding::NON_ALPHANUMERIC)
                .to_string()
                .replace("%2F", "/")
                .replace("%3A", ":")
        )
    }

    #[test]
    fn only_a_shown_and_unchanged_html_file_attaches() {
        let home = tempfile::tempdir().unwrap();
        let store = store(home.path());
        let mockups = home.path().join("docs/mockups");
        fs::create_dir_all(&mockups).unwrap();
        let mock = mockups.join("m.html");
        fs::write(&mock, DOC).unwrap();
        let md = home.path().join("r.md");
        fs::write(&md, "# 보고서\n").unwrap();
        let unchecked = |path: &Path| Err(Refusal::UncheckedDoc(path.display().to_string()));
        assert_eq!(attach_in(&store, &md), unchecked(&md));
        assert_eq!(attach_in(&store, &mock), unchecked(&mock));
        assert!(stamp_in(&store, &md).is_err());

        assert!(pass_in(&store, &mock).is_err());
        let shown = stamp_in(&store, &mock).unwrap();
        assert_eq!(attach_in(&store, &mock), unchecked(&mock));
        assert_eq!(pass_in(&store, &mock).unwrap(), shown);
        assert_eq!(attach_in(&store, &mock), Ok(shown.clone()));
        let link = |path: &Path| format!("[목업]({})", file_url(path));
        assert_eq!(
            unchecked_link_in(&store, &link(&mock), Path::new("/")),
            None
        );
        assert_eq!(
            unchecked_link_in(
                &store,
                &format!("{} {}", link(&mock), link(&md)),
                Path::new("/")
            ),
            Some(file_url(&md))
        );
        assert_eq!(
            unchecked_link_in(&store, "[웹](https://a.b/c) (x)", Path::new("/")),
            None
        );

        let changed = fs::read_to_string(&mock).unwrap().replace("총괄", "코디");
        fs::write(&mock, changed).unwrap();
        assert_eq!(attach_in(&store, &mock), unchecked(&mock));
        assert_eq!(shown_in(&store, &mock), shown);
        assert_eq!(attach_in(&store, &mock), Ok(shown));
        assert!(
            Refusal::UncheckedDoc("x".into())
                .to_string()
                .contains("--doc takes only a user-doc HTML file that passed show.sh.")
        );
    }

    #[test]
    fn a_link_to_the_doc_becomes_the_doc_link() {
        let dir = tempfile::tempdir().unwrap();
        let doc = dir.path().join("목업 a.html");
        let other = dir.path().join("b.html");
        fs::write(&doc, "").unwrap();
        fs::write(&other, "").unwrap();
        let url = file_url(&doc);
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
    fn only_html_files_are_docs() {
        assert!(html(Path::new("/r/a.HTML")));
        assert!(html(Path::new("/r/a.htm")));
        for name in [
            "/r/audit.md",
            "/r/run.log",
            "/r/README",
            "/r/rows.csv",
            "/r/a.svg",
        ] {
            assert!(!html(Path::new(name)), "{name}");
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
        let broken = "/a\tR1\n/b\tR3/c\tR2\n/d\tR3\n/e\tR3\n\tR9\n";
        assert_eq!(lookup(broken, "/x", false), ("R4".into(), true));
        assert_eq!(lookup(broken, "/d", false), ("R3".into(), false));
    }

    #[test]
    fn concurrent_registration_gives_unique_ids_and_whole_rows() {
        let home = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(store(home.path()));
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
        let store = store(home.path());
        let repo = home.path().join("작업");
        fs::create_dir_all(repo.join("docs/mockups")).unwrap();
        let mock = repo.join("docs/mockups/m.html");
        fs::write(&mock, DOC).unwrap();
        let report = repo.join("r.html");
        let report_text = |title: &str| {
            DOC.replace("문서", title)
                .replace("</body>", "<svg></svg></body>")
        };
        fs::write(&report, report_text("감사")).unwrap();
        fs::write(repo.join("old.html"), "").unwrap();
        let old = store
            .doc_id(&real_path(&repo.join("old.html")).unwrap())
            .unwrap();
        fs::remove_file(repo.join("old.html")).unwrap();
        shown_in(&store, &mock);
        shown_in(&store, &report);
        let m = attach_in(&store, &mock).unwrap().id;
        let r = attach_in(&store, &report).unwrap().id;
        assert_eq!((old.as_str(), m.as_str(), r.as_str()), ("R1", "M1", "R2"));

        fs::write(&report, report_text("감사 2")).unwrap();
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
        fs::remove_file(&mock).unwrap();
        let DocFile { path: copy, folder } = store.doc_file(&m).unwrap();
        assert_eq!(folder.as_deref(), copy.parent());
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
            ("old.html", false, false)
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
