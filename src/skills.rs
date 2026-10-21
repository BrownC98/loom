//! loom이 싣는 스킬(`skills/`)과 모든 노드가 늘 보는 지시(STE-100, 랩터-3 「작업 중」, o-mode).
//! 노드 에이전트는 사람이 스킬을 설치하지 않아도 이 스킬을 쓴다. 바뀌는 이유는 스킬 글과 그 배포 방식이다.
//! 근거: docs/design.md 「내장 스킬」.

use std::io;
use std::path::{Path, PathBuf};

use rust_embed::Embed;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Embed)]
#[folder = "skills"]
struct Files;

/// Claude 플러그인 표지. 플러그인 스킬은 `loom:<스킬 이름>`으로 뜬다.
const MANIFEST: &str = r#"{"name":"loom","description":"loom이 싣는 스킬"}"#;

/// 데몬이 데이터 폴더에 푼 스킬 폴더들. 설정의 스킬 조합(`AlwaysOn`)마다 폴더가 하나다.
/// Claude·Cursor는 폴더를 플러그인으로(`--plugin-dir`), Codex·OpenCode 2는 그 안의 `skills/`를 스킬 폴더로 읽는다.
pub struct Plugin(PathBuf);

impl Plugin {
    /// 조합마다 폴더를 지우고 이 판의 스킬로 다시 쓴다. 데몬이 시작할 때 한 번 부른다. `data`는 데이터 폴더다.
    /// 모든 조합을 먼저 쓰는 이유: 노드를 띄울 때 쓰면 동시에 뜨는 노드끼리 같은 폴더를 쓴다. 조합은 16개, 폴더 하나는 약 80KB다.
    /// 지우는 이유: 새 판에서 없앤 파일이 남으면 노드가 옛 글을 읽는다.
    /// 지우지 못하면(Windows에서 노드가 연 파일) 덮어 쓴다. 그때는 없앤 파일만 남는다.
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

    /// 모든 조합의 폴더. 데몬이 Cursor 표지를 조합마다 더 쓴다.
    pub fn dirs(&self) -> impl Iterator<Item = PathBuf> + '_ {
        AlwaysOn::all().map(|on| self.dir(on))
    }

    /// 켠 스킬만 든 폴더. 끈 스킬이 있으면 `plugin-off/<끈 스킬>`이다. 모두 켠 조합은 옛 판과 같은 `plugin`이다. 판을 올려도 이미 뜬 노드가 그 폴더를 계속 읽는다.
    pub fn dir(&self, on: AlwaysOn) -> PathBuf {
        let off: Vec<&str> = SKILLS.into_iter().filter(|s| !on.has(s)).collect();
        match off.is_empty() {
            true => self.0.join("plugin"),
            false => self.0.join("plugin-off").join(off.join("+")),
        }
    }

    /// 스킬마다 `<이름>/SKILL.md`가 있는 폴더.
    pub fn skills(&self, on: AlwaysOn) -> PathBuf {
        self.dir(on).join("skills")
    }
}

/// 실은 스킬 폴더 이름. 설정이 하나씩 켜고 끈다. 새 스킬을 더하면 `AlwaysOn`에도 더한다(테스트가 막는다).
const SKILLS: [&str; 4] = ["o-mode", "user-doc", "ste-100", "raptor-3"];

/// 실은 파일 경로(`o-mode/SKILL.md`)의 스킬 이름.
fn skill_of(name: &str) -> &str {
    name.split('/').next().unwrap_or(name)
}

fn write(path: &Path, data: &[u8]) -> io::Result<()> {
    std::fs::create_dir_all(path.parent().expect("파일 경로에는 부모가 있다"))?;
    std::fs::write(path, data)
}

/// 노드가 시작할 때 그래프에서 선 자리. 시작 지시(`always_on`)를 고른다.
/// 뒤에 부모가 바뀌어도(`loom adopt`, `loom rm`의 손자 옮기기) 이미 뜬 노드의 시작 지시는 그대로다.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Position {
    /// 부모가 없다. 사람이 띄운 노드이고 o-mode 총괄이다.
    Root,
    /// 부모가 `loom spawn`으로 띄웠다. 역할은 브리프가 정한다.
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

/// 옛 설정에는 없다. 없으면 모두 켜짐이다.
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
            _ => unreachable!("`SKILLS`에 없는 스킬 폴더: {skill}"),
        }
    }

    /// 켜고 끈 모든 조합.
    fn all() -> impl Iterator<Item = Self> {
        (0..16u8).map(|bits| Self {
            o_mode: bits & 1 != 0,
            user_doc: bits & 2 != 0,
            ste100: bits & 4 != 0,
            raptor3: bits & 8 != 0,
        })
    }
}

/// 노드를 띄울 때 시작 지시를 고르는 값. 띄울 때 한 번 정하고, 뒤에 설정이나 부모가 바뀌어도 뜬 노드는 그대로다.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Launch {
    pub position: Position,
    pub skills: AlwaysOn,
}

/// 모든 노드의 시작 지시에 붙이는 글: STE-100 본문, 랩터-3 「작업 중」 절, o-mode 글. 설정이 끈 스킬의 글은 뺀다.
/// 부모 없는 노드는 o-mode 절차 전체, 자식은 브리프 공통 규칙을 받는다. 자식에게 총괄 절차를 주지 않는다.
/// 링크만 주면 모델이 읽지 않고 지나친다. 그래서 본문을 그대로 넣는다.
pub fn always_on(launch: Launch) -> String {
    let mut parts = Vec::new();
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

/// 시작 지시에 넣은 o-mode 글의 상대 링크는 시작 지시 안에서 열리지 않는다.
const MODE_NOTE: &str = "아래 o-mode 글은 늘 적용한다. 상대 링크는 내장 스킬 `o-mode` 폴더 기준이다. 링크한 절이 필요하면 그 스킬을 불러 읽는다.";

/// 머리말(`---` 사이)을 뺀 본문.
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
            assert!(text.starts_with("# STE-100\n"), "{text}");
            assert!(!text.contains("name: ste-100"));
            assert!(text.contains("\n# 랩터-3 작업 중\n"));
            assert!(text.contains("완료 보고에는 `랩터-3:` 한 줄로"));
            assert!(!text.contains("## 화면(UI/UX) 감사"));
            assert!(!text.contains("## 5단계"));
            assert!(!text.contains("name: o-mode"));
        }
    }

    /// 부모 없는 노드는 총괄 절차, 자식은 브리프 공통 규칙만 받는다.
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

    /// 끈 스킬의 글만 빠진다.
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
        assert!(no_ste.starts_with("# 랩터-3 작업 중\n"), "{no_ste}");
        let no_raptor = off(AlwaysOn {
            raptor3: false,
            ..on
        });
        assert!(no_raptor.starts_with("# STE-100\n"));
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
        assert!(mode_only.starts_with(MODE_NOTE), "{mode_only}");
        let none = off(AlwaysOn::all().next().unwrap());
        assert_eq!(none, "");
    }

    /// 옛 설정 줄에는 이 값이 없다. 없으면 켜짐이다.
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
        for skill in ["o-mode", "ste-100", "user-doc", "raptor-3"] {
            assert!(
                plugin.skills(on).join(skill).join("SKILL.md").is_file(),
                "{skill}"
            );
        }
        assert!(plugin.skills(on).join("o-mode/agent-rules.md").is_file());
        // 사람은 Orca와 loom 모두 `o-mode`로 부른다. loom 노드에서는 이 스킬이 그 이름을 받는다.
        let loom_mode = std::fs::read_to_string(plugin.skills(on).join("o-mode/SKILL.md")).unwrap();
        assert!(loom_mode.contains("\nname: o-mode\n"));
        assert!(plugin.skills(on).join("user-doc/scripts/show.sh").is_file());
    }

    /// 조합마다 폴더가 다르고, 그 폴더에는 켠 스킬만 있다.
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
        }
    }

    /// 실은 스킬 폴더마다 설정 스위치가 있다.
    #[test]
    fn every_shipped_skill_has_a_switch() {
        let shipped: std::collections::BTreeSet<_> =
            Files::iter().map(|n| skill_of(&n).to_owned()).collect();
        let switched: std::collections::BTreeSet<_> =
            SKILLS.iter().map(|s| s.to_string()).collect();
        assert_eq!(shipped, switched);
    }

    /// 실은 스킬은 같이 실은 파일만 가리킨다. 사람의 `~/.agents/skills`나 싣지 않은 스킬로 가는 링크가 없다.
    #[test]
    fn skills_link_only_to_shipped_files() {
        for name in Files::iter().filter(|n| n.ends_with(".md")) {
            let body = text(&name);
            assert!(!body.contains("~/.agents/"), "{name}");
            let base = Path::new(name.as_ref()).parent().unwrap();
            for target in links(&body) {
                if target.starts_with("http") {
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

    /// markdown 링크 `](대상)`의 대상들.
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
