//! `loom show`와 `--doc`(`loom ask`, `loom done`): user-doc 스킬의 문서를 점검하고 문서 ID를 붙인다. 사람의 브라우저는 열지 않는다.
//! 사람은 질문 줄의 문서 링크로 연다(2026-10-21 사람 지시 "목업이나 보고서 승인도 질문탭에 링크로 달아두는 식으루 구현하라, 직접 브라우저에 띄우지 말고").
//! bash가 없는 Windows 노드에서도 같은 일을 하도록 Rust에 둔다. 렌더 그림과 말풍선 점검은 `show.sh`가 한다.
//! `--doc`은 `loom show`가 점검을 지났다고 남긴 내용 그대로인 HTML만 받는다(R14 5절, 2026-10-23 Q687 A).
//! 근거: skills/user-doc/SKILL.md 「보이기」.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use ts_rs::TS;

use crate::graph::{Refusal, WaitDoc};
use crate::link_text::DOC_LINK;

/// 문서 머리 줄과 창 제목에 붙는 ID의 틀. 다시 열면 이 틀을 찾아 바꾼다.
const ID_OPEN: &str = r#"<span class="doc-id">"#;
const WHO: &str = r#"<div class="who">"#;

/// `loom show`: 점검하고 ID를 붙인다. 통과 기록은 `show.sh`가 렌더·말풍선 점검을 지난 뒤 `loom show --checked`로 남긴다.
/// bash가 없는 Windows 노드는 `show.sh`를 돌리지 못한다. 그 노드에서만 이 점검으로 통과를 남긴다(R14 5절, 2026-10-23 Q687 A).
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

/// `loom show --checked`: `show.sh`의 마지막 단계. 다시 점검하고, ID를 붙인 내용 그대로면 통과로 남긴다.
pub fn record_checked(path: &Path) -> Result<(), String> {
    let doc = pass_in(&Store::here()?, path)?;
    println!("show: 문서 ID {} 통과", doc.id);
    println!(
        "show: 사람에게 보이려면 질문에 붙인다: loom ask \"<질문>\" --doc {}",
        path.display()
    );
    Ok(())
}

/// WSL 노드의 `loom`도 Windows 실행 파일이지만 시작 폴더가 WSL 폴더다. 그 노드는 bash로 `show.sh`를 돌린다.
fn bashless() -> bool {
    cfg!(windows)
        && std::env::current_dir()
            .ok()
            .and_then(|dir| crate::wsl::of(&dir))
            .is_none()
}

/// `--doc`이 받는 꼴. 그 밖의 파일(`.md` 보고서, 그림, `.svg`)은 받지 않는다. 사람에게 보일 글은 user-doc HTML로 쓴다.
/// 근거: 사람에게 날 `.md`가 링크로 보였다(2026-10-23 사람 "질문에 올린 링크가 쌩 마크다운임").
const HTML_EXTENSIONS: [&str; 2] = ["html", "htm"];

fn html(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| HTML_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// 문서를 점검하고 ID를 붙인다. 같은 문서는 판이 바뀌어도 같은 ID다.
fn stamp_in(store: &Store, path: &Path) -> Result<WaitDoc, String> {
    let (path, text) = checked(path)?;
    let id = store.doc_id(&path)?;
    let title = bare_title(title(&text).unwrap_or_default()).to_owned();
    fs::write(&path, stamp(&text, &id)).map_err(|e| e.to_string())?;
    Ok(WaitDoc { id, title })
}

/// ID를 붙인 뒤 렌더·말풍선 점검을 지난 내용을 `--doc` 통과로 남긴다. 점검한 내용과 남기는 내용이 같도록 ID를 다시 붙이지 않는다.
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

/// `loom show`의 글 점검을 지난 문서의 실제 경로와 글.
fn checked(path: &Path) -> Result<(PathBuf, String), String> {
    let path = real_path(path)?;
    let text = read_html(&path)?;
    check(&text)?;
    if !mockup(&path) {
        check_report(&text)?;
    }
    Ok((path, text))
}

/// HTML 꼴이고 UTF-8로 읽히는 파일의 글.
fn read_html(path: &Path) -> Result<String, String> {
    // 총괄이 그림(PNG)을 붙여 「stream did not contain valid UTF-8」만 받았다(2026-10-22).
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

/// `--doc`(`loom ask`, `loom done`)의 문서에 ID를 붙인다. `show.sh`가 점검한 뒤 바뀌지 않은 HTML만 받는다.
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
    store.keep(&id, &path, &text).map_err(Refusal::Db)?;
    let title = bare_title(title(&text).unwrap_or_default()).to_owned();
    Ok(WaitDoc { id, title })
}

/// 사람이 읽는 글(부모 없는 노드의 질문·완료) 속 `file://` 링크 가운데 `--doc`이 받지 않을 파일을 처음 하나 돌려준다.
/// 붙인 문서의 링크는 CLI가 이미 `DOC_LINK`로 바꿨다(`inline_link`). `folder`는 데몬이 아는 노드 폴더다. 경로는 그 노드 쪽에서 푼다.
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
    /// `$HOME/.cache/user-doc/passed.txt`: 점검을 지난 문서 내용의 해시(`skills::digest`)를 한 줄씩. 줄을 더하기만 한다.
    passes: PathBuf,
    /// `<데이터 폴더>/docs`. 데몬 DB(`db::data_dir`) 옆이다.
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
        // 한 줄을 쓰기 한 번에 쓴다. 동시에 남겨도 줄이 섞이지 않는다.
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
        // 잠금 전에 겹친 ID는 첫 줄만 둔다. `doc_file`도 첫 줄을 연다.
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

    /// `<데이터 폴더>/docs/<ID>/<원본 파일 이름>`. 이름이 같아 꼴(`html`)도 같다.
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

/// 링크 주소가 가리키는 파일 경로. `file:///C:/…`는 `C:/…`, `file://host/…`는 UNC `//host/…`다. 조각(`#…`)과 물음(`?…`)은 파일 밖이다.
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
    if let Some((_, missing)) = marks.iter().find(|(mark, _)| !html.contains(mark)) {
        return Err((*missing).into());
    }
    match local_refs(html).first() {
        Some(r) => Err(format!(
            "한 파일이 아니다: {r}. 그림은 data: 주소나 <svg>로 문서 안에 넣는다"
        )),
        None => Ok(()),
    }
}

/// 문서 밖 파일을 가리키는 주소. `check-callouts.cjs`의 `LOCAL_REFS`와 같은 규칙을 글로 본다(`<script>` 밖의 속성, `<style>`과 `style` 속성의 `url()`·`@import`).
/// 근거: `--doc` 통과 기록은 `loom show`가 남기므로, `show.sh` 없이 부른 `loom show`도 옆 그림이 빠진 문서를 거절해야 한다(2026-10-23 검토 추적-codex).
fn local_refs(html: &str) -> Vec<String> {
    let one_file = |v: &str| {
        let v = v.trim().to_ascii_lowercase();
        v.is_empty() || ["data:", "https:", "#"].iter().any(|p| v.starts_with(p))
    };
    let mut bad = Vec::new();
    let markup = without_blocks(html, "script");
    for attr in ["src", "href", "srcset", "poster"] {
        for value in attr_values(&markup, attr) {
            let refs = if attr == "srcset" {
                srcset_urls(&value)
            } else {
                vec![value.clone()]
            };
            bad.extend(
                refs.into_iter()
                    .filter(|r| !one_file(r))
                    .map(|r| format!("{attr}=\"{r}\"")),
            );
        }
    }
    let mut css = blocks(html, "style");
    css.extend(attr_values(&markup, "style"));
    for text in css {
        for (open, end) in [
            ("url(", &[')', '\'', '"'][..]),
            ("@import", &[';', ' ', '\'', '"', ')'][..]),
        ] {
            let lower = text.to_ascii_lowercase();
            for (at, _) in lower.match_indices(open) {
                let rest = text[at + open.len()..]
                    .trim_start()
                    .trim_start_matches("url(")
                    .trim_start_matches(['\'', '"']);
                let url = rest.split(end).next().unwrap_or_default();
                if !one_file(url) {
                    bad.push(format!("{open}{url}"));
                }
            }
        }
    }
    bad
}

/// `<tag …>…</tag>` 안의 글.
fn blocks(html: &str, tag: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let (open, close) = (format!("<{tag}"), format!("</{tag}"));
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(start) = lower[from..].find(&open).map(|i| from + i) {
        let Some(body) = lower[start..].find('>').map(|i| start + i + 1) else {
            break;
        };
        let end = lower[body..].find(&close).map_or(html.len(), |i| body + i);
        out.push(html[body..end].to_owned());
        from = end;
    }
    out
}

/// `<tag …>…</tag>`를 뺀 글.
fn without_blocks(html: &str, tag: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let (open, close) = (format!("<{tag}"), format!("</{tag}>"));
    let mut out = String::new();
    let mut from = 0;
    while let Some(start) = lower[from..].find(&open).map(|i| from + i) {
        out.push_str(&html[from..start]);
        from = lower[start..]
            .find(&close)
            .map_or(html.len(), |i| start + i + close.len());
    }
    out.push_str(&html[from..]);
    out
}

/// 빈칸 뒤의 `attr=` 속성 값. 따옴표가 없으면 빈칸이나 `>`까지다.
fn attr_values(markup: &str, attr: &str) -> Vec<String> {
    let lower = markup.to_ascii_lowercase();
    let key = format!("{attr}=");
    lower
        .match_indices(&key)
        .filter(|(at, _)| markup[..*at].ends_with(|c: char| c.is_ascii_whitespace()))
        .map(|(at, _)| {
            let rest = &markup[at + key.len()..];
            match rest.chars().next() {
                Some(q @ ('"' | '\'')) => rest[1..].split(q).next().unwrap_or_default(),
                _ => rest
                    .split(|c: char| c.is_ascii_whitespace() || c == '>')
                    .next()
                    .unwrap_or_default(),
            }
            .to_owned()
        })
        .collect()
}

/// `srcset`의 후보 주소. 후보는 「설명자(`2x`, `640w`) 뒤 쉼표」나 「쉼표+빈칸」으로 나뉜다. `data:` 주소 안 쉼표 뒤에는 빈칸이 없다.
fn srcset_urls(srcset: &str) -> Vec<String> {
    let descriptor = |piece: &str| {
        piece
            .trim_end()
            .rsplit_once(char::is_whitespace)
            .is_some_and(|(_, d)| {
                d.len() > 1 && d.ends_with(['w', 'x']) && d[..d.len() - 1].parse::<f64>().is_ok()
            })
    };
    let mut candidates = vec![String::new()];
    let mut pieces = srcset.split(',').peekable();
    while let Some(piece) = pieces.next() {
        let current = candidates.last_mut().expect("목록은 비지 않는다");
        current.push_str(piece);
        let next_starts = pieces
            .peek()
            .is_some_and(|n| n.starts_with(char::is_whitespace));
        if pieces.peek().is_some() {
            if descriptor(current) || next_starts {
                candidates.push(String::new());
            } else {
                current.push(',');
            }
        }
    }
    candidates
        .iter()
        .filter_map(|c| c.split_whitespace().next().map(str::to_owned))
        .collect()
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
        // 다른 ID 표에서 붙은 ID는 바꾼다.
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

    /// `loom show`도 `check-callouts.cjs`처럼 문서 밖 파일을 가리키는 주소를 거절한다.
    #[test]
    fn a_doc_must_be_one_file() {
        let doc = |extra: &str| DOC.replace("</body>", &format!("{extra}</body>"));
        for one in [
            r##"<a href="#part">x</a>"##,
            r#"<img src="data:image/png;base64,AAA,BBB">"#,
            r#"<img srcset="data:image/png;base64,A,B 1x, https://a.b/c.png 2x">"#,
            r#"<a href="https://a.b/c">x</a><div data-src="local.png"></div>"#,
            r#"<style>.a{background:url("data:image/svg+xml,%3Csvg%3E")}</style>"#,
            r#"<script>el.src = "local.png"; x = ' src="a.png"';</script>"#,
        ] {
            assert_eq!(local_refs(&doc(one)), Vec::<String>::new(), "{one}");
        }
        for local in [
            r#"<img src="missing.png">"#,
            r#"<img src=missing.png>"#,
            r#"<img srcset="data:image/png;base64,A 1x, local@2x.png 2x">"#,
            r#"<a href="other.html">x</a>"#,
            r#"<a href="http://a.b/c">x</a>"#,
            r#"<video poster="p.jpg"></video>"#,
            r#"<div style="background: url(bg.png)"></div>"#,
            r#"<style>@import "local.css";</style>"#,
        ] {
            assert_eq!(local_refs(&doc(local)).len(), 1, "{local}");
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.html");
        fs::write(&path, doc(r#"<img src="missing.png">"#)).unwrap();
        let shown = stamp_in(&store(dir.path()), &path);
        assert!(shown.is_err_and(|e| e.contains("missing.png")));
    }

    /// 점검을 지난 문서의 조각 링크(`#part`)도 그 문서다.
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

    /// `show.sh`의 두 단계(`loom show`, `loom show --checked`)를 지난 문서.
    fn shown_in(store: &Store, path: &Path) -> WaitDoc {
        stamp_in(store, path).unwrap();
        pass_in(store, path).unwrap()
    }

    /// 경로의 `file://` 주소. Windows 경로는 `file:///C:/…`다.
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

    /// `--doc`은 `loom show`가 지난 내용 그대로인 HTML만 받는다. 글 파일, 점검하지 않은 HTML, 점검 뒤 바뀐 HTML은 거절한다.
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

        // `loom show`만으로는 통과가 아니다. 렌더·말풍선 점검 뒤 `--checked`가 남긴다.
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
        // 잠금 전에 깨진 표: 섞인 줄은 건너뛰고, 새 번호는 가장 큰 번호 다음이다.
        let broken = "/a\tR1\n/b\tR3/c\tR2\n/d\tR3\n/e\tR3\n\tR9\n";
        assert_eq!(lookup(broken, "/x", false), ("R4".into(), true));
        assert_eq!(lookup(broken, "/d", false), ("R3".into(), false));
    }

    /// 여러 스레드가 동시에 붙여도 ID가 겹치지 않고 줄이 깨지지 않는다.
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
        // 이 판 전에 붙인 문서: 표에만 있고 사본이 없다.
        let old = store
            .doc_id(&real_path(&repo.join("old.html")).unwrap())
            .unwrap();
        fs::remove_file(repo.join("old.html")).unwrap();
        shown_in(&store, &mock);
        shown_in(&store, &report);
        let m = attach_in(&store, &mock).unwrap().id;
        let r = attach_in(&store, &report).unwrap().id;
        assert_eq!((old.as_str(), m.as_str(), r.as_str()), ("R1", "M1", "R2"));

        // 원본이 있으면 원본을 낸다(새 판이 보인다). 원본 보고서는 옆 파일을 내지 않는다. 원본 목업은 그 폴더다.
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
