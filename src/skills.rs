use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use rust_embed::{Embed, EmbeddedFile};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

#[derive(Embed)]
#[folder = "skills"]
struct Files;

pub struct Plugin(PathBuf);

const HASHES: &str = "skills.sha256";

impl Plugin {
    pub fn install(data: &Path) -> io::Result<Self> {
        let plugin = Self(data.to_owned());
        let now: BTreeMap<String, String> = Files::iter()
            .map(|name| {
                let file = Files::get(&name).expect("iter가 준 이름은 늘 있다");
                (name.into_owned(), hex(&file.metadata.sha256_hash()))
            })
            .collect();
        let hashes = data.join(HASHES);
        let written = std::fs::read_to_string(&hashes)
            .ok()
            .map(|text| read_hashes(&text))
            .filter(|_| AlwaysOn::all().all(|on| plugin.skills(on).is_dir()));
        if written.as_ref() == Some(&now) {
            return Ok(plugin);
        }
        let pruned = std::thread::scope(|scope| {
            let syncs: Vec<_> = AlwaysOn::all()
                .map(|on| {
                    let skills = plugin.skills(on);
                    let (written, now) = (written.as_ref(), &now);
                    scope.spawn(move || sync(&skills, on, written, now))
                })
                .collect();
            syncs
                .into_iter()
                .map(|s| s.join().expect("스킬 폴더 스레드는 패닉하지 않는다"))
                .collect::<io::Result<Vec<bool>>>()
        })?;
        if !pruned.iter().all(|p| *p) {
            return Ok(plugin);
        }
        std::fs::write(
            hashes,
            now.iter()
                .map(|(name, hash)| format!("{hash}  {name}\n"))
                .collect::<String>(),
        )?;
        Ok(plugin)
    }

    pub fn dirs(&self) -> impl Iterator<Item = PathBuf> + '_ {
        AlwaysOn::all().map(|on| self.dir(on))
    }

    pub fn dir(&self, on: AlwaysOn) -> PathBuf {
        let off: Vec<&str> = SKILLS.into_iter().filter(|s| !on.has(s)).collect();
        match off.is_empty() {
            true => self.0.join("plugin"),
            false => self.0.join("plugin-off").join(off.join("+")),
        }
    }

    pub fn skills(&self, on: AlwaysOn) -> PathBuf {
        self.dir(on).join("skills")
    }
}

pub fn digest(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sync(
    skills: &Path,
    on: AlwaysOn,
    written: Option<&BTreeMap<String, String>>,
    now: &BTreeMap<String, String>,
) -> io::Result<bool> {
    let keep: BTreeMap<PathBuf, (String, EmbeddedFile)> = Files::iter()
        .filter(|name| on.has(skill_of(name)))
        .map(|name| {
            let file = Files::get(&name).expect("iter가 준 이름은 늘 있다");
            (skills.join(name.as_ref()), (name.into_owned(), file))
        })
        .collect();
    let pruned = prune(skills, &keep);
    if let Err(e) = &pruned {
        eprintln!(
            "loom: {}의 낡은 스킬 파일을 지우지 못했다: {e}",
            skills.display()
        );
    }
    for (path, (name, file)) in &keep {
        let same = match written {
            Some(written) => written.get(name) == now.get(name) && path.exists(),
            None => std::fs::read(path).ok().as_deref() == Some(file.data.as_ref()),
        };
        if !same {
            write(path, &file.data)?;
        }
    }
    Ok(pruned.is_ok())
}

fn read_hashes(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once("  "))
        .map(|(hash, name)| (name.to_owned(), hash.to_owned()))
        .collect()
}

const SKILLS: [&str; 4] = ["o-mode", "user-doc", "ste-orwell", "raptor-3"];

const ASK: &str = "ask-human";

const DESIGN: &str = "design-stack";

pub fn names(on: AlwaysOn) -> impl Iterator<Item = &'static str> {
    [ASK, DESIGN]
        .into_iter()
        .chain(SKILLS.into_iter().filter(move |s| on.has(s)))
}

fn skill_of(name: &str) -> &str {
    name.split('/').next().unwrap_or(name)
}

fn prune<V>(dir: &Path, keep: &BTreeMap<PathBuf, V>) -> io::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        entries => entries?,
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            prune(&path, keep)?;
            if std::fs::read_dir(&path)?.next().is_none() {
                std::fs::remove_dir(&path)?;
            }
        } else if !keep.contains_key(&path) {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

fn write(path: &Path, data: &[u8]) -> io::Result<()> {
    std::fs::create_dir_all(path.parent().expect("파일 경로에는 부모가 있다"))?;
    std::fs::write(path, data)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Position {
    Root,
    Child,
}

/// 설정에서 켜고 끄는 내장 스킬. 끈 스킬은 새로 띄우는 노드에 주지 않는다: 시작 지시에서 글을 빼고 스킬 폴더에서도 뺀다.
/// 근거: 목업 docs/mockups/skill-options.html, 2026-10-21 사람 지시 "o-mode·user-doc 도 옵션에 추가해".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default, rename_all = "camelCase")]
#[ts(export)]
pub struct AlwaysOn {
    pub o_mode: bool,
    pub user_doc: bool,
    #[serde(alias = "ste100")]
    pub ste_orwell: bool,
    pub raptor3: bool,
}

impl Default for AlwaysOn {
    fn default() -> Self {
        Self {
            o_mode: true,
            user_doc: true,
            ste_orwell: true,
            raptor3: true,
        }
    }
}

impl AlwaysOn {
    fn has(self, skill: &str) -> bool {
        match skill {
            "o-mode" => self.o_mode,
            "user-doc" => self.user_doc,
            "ste-orwell" => self.ste_orwell,
            "raptor-3" => self.raptor3,
            ASK | DESIGN => true,
            _ => unreachable!("`SKILLS`에 없는 스킬 폴더: {skill}"),
        }
    }

    fn all() -> impl Iterator<Item = Self> {
        (0..16u8).map(|bits| Self {
            o_mode: bits & 1 != 0,
            user_doc: bits & 2 != 0,
            ste_orwell: bits & 4 != 0,
            raptor3: bits & 8 != 0,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Launch {
    pub position: Position,
    pub skills: AlwaysOn,
}

pub fn always_on(launch: Launch) -> String {
    parts(launch)
        .into_iter()
        .map(|source| part(source).expect("`parts`가 고른 원본은 늘 있다"))
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub fn part_digests(launch: Launch) -> BTreeMap<String, String> {
    parts(launch)
        .into_iter()
        .filter_map(|source| Some((source.to_owned(), part_digest(source)?)))
        .collect()
}

pub fn part_digest(source: &str) -> Option<String> {
    part(source).map(|text| digest(&[&text]))
}

fn parts(launch: Launch) -> Vec<&'static str> {
    let mode = match launch.position {
        Position::Root => MODE_ROOT,
        Position::Child => MODE_CHILD,
    };
    [
        (true, ASK_SOURCE),
        (launch.skills.ste_orwell, STE_SOURCE),
        (launch.skills.raptor3, RAPTOR_SOURCE),
        (launch.skills.o_mode, mode),
    ]
    .into_iter()
    .filter_map(|(on, source)| on.then_some(source))
    .collect()
}

const ASK_SOURCE: &str = "ask-human/SKILL.md";
const STE_SOURCE: &str = "ste-orwell/SKILL.md";
const RAPTOR_SOURCE: &str = "raptor-3/SKILL.md";
const MODE_ROOT: &str = "o-mode/SKILL.md";
const MODE_CHILD: &str = "o-mode/agent-rules.md";

fn part(source: &str) -> Option<String> {
    match source {
        ASK_SOURCE => Some(ask_rules()),
        STE_SOURCE => Some(body(&text(source)).to_owned()),
        RAPTOR_SOURCE => {
            let raptor = text(source);
            let (_, rest) = raptor
                .split_once("\n## While you work\n")
                .expect("랩터-3에 「While you work」 절이 있다");
            let section = rest.split("\n## ").next().unwrap_or(rest);
            Some(format!("# Raptor-3 while you work\n\n{}", section.trim()))
        }
        MODE_ROOT | MODE_CHILD => Some(format!("{MODE_NOTE}\n\n{}", body(&text(source)))),
        _ => None,
    }
}

pub fn ask_rules() -> String {
    body(&text(ASK_SOURCE)).to_owned()
}

const MODE_NOTE: &str = "The o-mode text below always applies. Its relative links start in the built-in skill folder `o-mode`.";

fn body(skill: &str) -> &str {
    skill
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map_or(skill, |(_, body)| body)
        .trim()
}

fn text(name: &str) -> String {
    let file = Files::get(name).expect("skills/ 폴더의 파일이다");
    String::from_utf8(file.data.into_owned()).expect("스킬 글은 UTF-8이다")
}
