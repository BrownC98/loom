use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::link_text::DOC_LINK;
use crate::protocol::{Refusal, WaitDoc};

mod share;
pub use share::{Shared, shareable, with_bridge};

const ID_OPEN: &str = r#"<span class="doc-id">"#;
const WHO: &str = r#"<div class="who">"#;

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DocStep {
    Stamp,
    Pass,
}

pub fn take_step(path: &Path, step: DocStep) -> Result<WaitDoc, String> {
    let store = Store::here()?;
    match step {
        DocStep::Stamp => stamp_in(&store, path),
        DocStep::Pass => pass_in(&store, path),
    }
}

pub fn show(path: &Path, step: DocStep) -> Result<(), String> {
    let doc = take_step(path, step)?;
    match (step, bashless()) {
        (DocStep::Stamp, true) => {
            println!("show: 문서 ID {} ({})", doc.id, doc.title);
            take_step(path, DocStep::Pass)?;
            println!("show: bash 없는 Windows 노드라 이 점검으로 통과를 남긴다");
            print_ask(path);
        }
        _ => told(&doc, step, path),
    }
    Ok(())
}

pub fn told(doc: &WaitDoc, step: DocStep, path: &Path) {
    match step {
        DocStep::Stamp => {
            println!("show: 문서 ID {} ({})", doc.id, doc.title);
            println!("show: --doc 통과는 show.sh가 렌더·말풍선 점검 뒤에 남긴다");
        }
        DocStep::Pass => {
            println!("show: 문서 ID {} 통과", doc.id);
            print_ask(path);
        }
    }
}

fn print_ask(path: &Path) {
    println!(
        "show: 사람에게 보이려면 질문에 붙인다: loom ask \"<질문>\" --doc {}",
        path.display()
    );
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

pub fn doc_label(text: &str, doc: &Path) -> Result<(), Refusal> {
    let Some(h1) = read_html(doc).ok().as_deref().and_then(h1) else {
        return Ok(());
    };
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let link = &rest[open..];
        match crate::link_text::parse_link(link) {
            Some((label, DOC_LINK, len)) => {
                if words(label) != h1 {
                    return Err(Refusal::DocLinkLabel {
                        label: label.to_owned(),
                        h1,
                    });
                }
                rest = &link[len..];
            }
            Some((_, _, len)) => rest = &link[len..],
            None => rest = &link[1..],
        }
    }
    Ok(())
}

fn h1(html: &str) -> Option<String> {
    let open = html.find("<h1")?;
    let start = open + html[open..].find('>')? + 1;
    let inner = &html[start..start + html[start..].find("</h1>")?];
    let mut bare = String::with_capacity(inner.len());
    let mut in_tag = false;
    for c in inner.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => bare.push(c),
            _ => {}
        }
    }
    let bare = entities(&bare).replace(['[', ']'], "");
    Some(words(&bare)).filter(|h1| !h1.is_empty())
}

fn entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let entity = rest.find(';').map(|end| (&rest[1..end], end + 1));
        let decoded = entity.and_then(|(name, len)| {
            let c = match name {
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "amp" => Some('&'),
                "nbsp" => Some(' '),
                _ => match name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok().and_then(char::from_u32),
                    None => name
                        .strip_prefix('#')
                        .and_then(|dec| dec.parse().ok())
                        .and_then(char::from_u32),
                },
            };
            c.map(|c| (c, len))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn words(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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
