use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use rust_embed::Embed;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

#[derive(Embed)]
#[folder = "skills"]
struct Files;

const MANIFEST: &str = r#"{"name":"loom","description":"loom이 싣는 스킬"}"#;

pub struct Plugin(PathBuf);

impl Plugin {
    pub fn install(data: &Path) -> io::Result<Self> {
        let plugin = Self(data.to_owned());
        for on in AlwaysOn::all() {
            let dir = plugin.dir(on);
            let _ = std::fs::remove_dir_all(&dir);
            write(
                &dir.join(".claude-plugin").join("plugin.json"),
                MANIFEST.as_bytes(),
            )?;
            for name in Files::iter().filter(|name| on.has(skill_of(name))) {
                let file = Files::get(&name).expect("iter가 준 이름은 늘 있다");
                write(&dir.join("skills").join(name.as_ref()), &file.data)?;
            }
        }
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

pub fn digests() -> BTreeMap<String, String> {
    let mut names: Vec<_> = Files::iter().collect();
    names.sort();
    let mut hashers: BTreeMap<String, Sha256> = BTreeMap::new();
    for name in names {
        let file = Files::get(&name).expect("iter가 준 이름은 늘 있다");
        let hasher = hashers.entry(skill_of(&name).to_owned()).or_default();
        hasher.update(name.as_bytes());
        hasher.update([0]);
        hasher.update(&file.data);
        hasher.update([0]);
    }
    hashers
        .into_iter()
        .map(|(skill, hasher)| (skill, hex(hasher)))
        .collect()
}

pub fn digest(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    hex(hasher)
}

fn hex(hasher: Sha256) -> String {
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

const SKILLS: [&str; 4] = ["o-mode", "user-doc", "ste-100", "raptor-3"];

const ASK: &str = "ask-human";

fn skill_of(name: &str) -> &str {
    name.split('/').next().unwrap_or(name)
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
    pub ste100: bool,
    pub raptor3: bool,
}

impl Default for AlwaysOn {
    fn default() -> Self {
        Self {
            o_mode: true,
            user_doc: true,
            ste100: true,
            raptor3: true,
        }
    }
}

impl AlwaysOn {
    fn has(self, skill: &str) -> bool {
        match skill {
            "o-mode" => self.o_mode,
            "user-doc" => self.user_doc,
            "ste-100" => self.ste100,
            "raptor-3" => self.raptor3,
            ASK => true,
            _ => unreachable!("`SKILLS`에 없는 스킬 폴더: {skill}"),
        }
    }

    fn all() -> impl Iterator<Item = Self> {
        (0..16u8).map(|bits| Self {
            o_mode: bits & 1 != 0,
            user_doc: bits & 2 != 0,
            ste100: bits & 4 != 0,
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
    let mut parts = vec![ask_rules()];
    if launch.skills.ste100 {
        parts.push(body(&text("ste-100/SKILL.md")).to_owned());
    }
    if launch.skills.raptor3 {
        let raptor = text("raptor-3/SKILL.md");
        let (_, rest) = raptor
            .split_once("\n## 작업 중\n")
            .expect("랩터-3에 「작업 중」 절이 있다");
        let section = rest.split("\n## ").next().unwrap_or(rest);
        parts.push(format!("# 랩터-3 작업 중\n\n{}", section.trim()));
    }
    if launch.skills.o_mode {
        let mode = match launch.position {
            Position::Root => text("o-mode/SKILL.md"),
            Position::Child => text("o-mode/agent-rules.md"),
        };
        parts.push(format!("{MODE_NOTE}\n\n{}", body(&mode)));
    }
    parts.join("\n\n")
}

pub fn ask_rules() -> String {
    body(&text(&format!("{ASK}/SKILL.md"))).to_owned()
}

const MODE_NOTE: &str = "아래 o-mode 글은 늘 적용한다. 상대 링크는 내장 스킬 `o-mode` 폴더 기준이다. 링크한 절이나 파일이 필요하면 그 스킬 폴더에서 읽는다.";

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

#[cfg(test)]
mod tests {
    use super::*;

    fn launch(position: Position) -> Launch {
        Launch {
            position,
            skills: AlwaysOn::default(),
        }
    }

    #[test]
    fn always_on_has_the_ste_body_and_only_the_raptor_work_section() {
        for position in [Position::Root, Position::Child] {
            let text = always_on(launch(position));
            assert!(text.starts_with("# 사람에게 묻기\n"), "{text}");
            assert!(text.contains("\n# STE-100\n"));
            assert!(!text.contains("name: ste-100"));
            assert!(text.contains("\n# 랩터-3 작업 중\n"));
            assert!(text.contains("완료 보고에는 `랩터-3:` 한 줄로"));
            assert!(!text.contains("## 화면(UI/UX) 감사"));
            assert!(!text.contains("## 5단계"));
            assert!(!text.contains("name: o-mode"));
            assert_eq!(text.matches("# 사람에게 묻기\n").count(), 1);
            assert!(!text.contains("name: ask-human"));
        }
    }

    #[test]
    fn root_gets_the_o_mode_procedure_and_child_gets_the_brief_rules() {
        let root = always_on(launch(Position::Root));
        assert!(root.contains("\n# Loom 병렬 개발\n"));
        assert!(root.contains("## 병합과 회수"));
        assert!(!root.contains("# Loom 브리프 공통 규칙"));
        let child = always_on(launch(Position::Child));
        assert!(child.contains("\n# Loom 브리프 공통 규칙\n"));
        assert!(!child.contains("# Loom 병렬 개발"));
        assert!(!child.contains("## 병합과 회수"));
    }

    #[test]
    fn switched_off_skills_leave_the_launch_text() {
        let off = |skills| {
            always_on(Launch {
                position: Position::Root,
                skills,
            })
        };
        let on = AlwaysOn::default();
        let no_ste = off(AlwaysOn {
            ste100: false,
            ..on
        });
        assert!(!no_ste.contains("# STE-100"));
        assert!(no_ste.contains("\n# 랩터-3 작업 중\n"), "{no_ste}");
        let no_raptor = off(AlwaysOn {
            raptor3: false,
            ..on
        });
        assert!(no_raptor.contains("\n# STE-100\n"));
        assert!(!no_raptor.contains("랩터-3 작업 중"));
        let no_mode = off(AlwaysOn {
            o_mode: false,
            ..on
        });
        assert!(!no_mode.contains(MODE_NOTE));
        assert!(!no_mode.contains("# Loom 병렬 개발"));
        assert!(no_mode.contains("# 랩터-3 작업 중\n"));
        let mode_only = off(AlwaysOn {
            o_mode: true,
            ..AlwaysOn::all().next().unwrap()
        });
        assert_eq!(
            mode_only,
            format!(
                "{}\n\n{MODE_NOTE}\n\n{}",
                ask_rules(),
                body(&text("o-mode/SKILL.md"))
            )
        );
        let none = off(AlwaysOn::all().next().unwrap());
        assert_eq!(none, ask_rules());
    }

    #[test]
    fn missing_switches_read_as_on() {
        let on: AlwaysOn = serde_json::from_str("{}").unwrap();
        assert_eq!(on, AlwaysOn::default());
        let one: AlwaysOn = serde_json::from_str(r#"{"raptor3":false}"#).unwrap();
        assert_eq!(
            one,
            AlwaysOn {
                raptor3: false,
                ..AlwaysOn::default()
            }
        );
    }

    #[test]
    fn install_writes_the_plugin_and_removes_old_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("plugin");
        write(&dir.join("skills/old/SKILL.md"), b"old").unwrap();
        let plugin = Plugin::install(tmp.path()).unwrap();
        let on = AlwaysOn::default();
        assert_eq!(plugin.dir(on), dir);
        assert!(!dir.join("skills/old").exists());
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join(".claude-plugin/plugin.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["name"], "loom");
        for skill in ["o-mode", "ste-100", "user-doc", "raptor-3", ASK] {
            assert!(
                plugin.skills(on).join(skill).join("SKILL.md").is_file(),
                "{skill}"
            );
        }
        assert!(plugin.skills(on).join("o-mode/agent-rules.md").is_file());
        let loom_mode = std::fs::read_to_string(plugin.skills(on).join("o-mode/SKILL.md")).unwrap();
        assert!(loom_mode.contains("\nname: o-mode\n"));
        assert!(plugin.skills(on).join("user-doc/scripts/show.sh").is_file());
    }

    #[test]
    fn each_combination_has_only_its_skills() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin = Plugin::install(tmp.path()).unwrap();
        let dirs: std::collections::HashSet<_> = plugin.dirs().collect();
        assert_eq!(dirs.len(), 16);
        for on in AlwaysOn::all() {
            assert!(plugin.dir(on).join(".claude-plugin/plugin.json").is_file());
            for skill in SKILLS {
                let file = plugin.skills(on).join(skill).join("SKILL.md");
                assert_eq!(file.is_file(), on.has(skill), "{on:?} {skill}");
            }
            assert!(plugin.skills(on).join(ASK).join("SKILL.md").is_file());
        }
    }

    #[test]
    fn every_shipped_skill_has_a_switch() {
        let shipped: std::collections::BTreeSet<_> =
            Files::iter().map(|n| skill_of(&n).to_owned()).collect();
        let switched: std::collections::BTreeSet<_> =
            SKILLS.iter().chain([&ASK]).map(|s| s.to_string()).collect();
        assert_eq!(shipped, switched);
    }

    #[test]
    fn skills_link_only_to_shipped_files() {
        for name in Files::iter().filter(|n| n.ends_with(".md")) {
            let body = text(&name);
            assert!(!body.contains("~/.agents/"), "{name}");
            let base = Path::new(name.as_ref()).parent().unwrap();
            for target in links(&body) {
                if target.starts_with("http") || target.starts_with('<') {
                    continue;
                }
                let file = target.split('#').next().unwrap();
                let path = normalize(&base.join(file));
                assert!(
                    Files::get(&path).is_some(),
                    "{name}: `{target}` is not shipped"
                );
            }
        }
    }

    fn links(body: &str) -> Vec<&str> {
        body.split("](")
            .skip(1)
            .filter_map(|rest| rest.split_once(')').map(|(target, _)| target))
            .collect()
    }

    fn normalize(path: &Path) -> String {
        let mut parts: Vec<&str> = Vec::new();
        for part in path.iter().map(|p| p.to_str().unwrap()) {
            match part {
                ".." => {
                    parts.pop();
                }
                "." => {}
                p => parts.push(p),
            }
        }
        parts.join("/")
    }
}
