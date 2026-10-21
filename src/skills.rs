//! loom이 싣는 스킬(`skills/`)과 모든 노드가 늘 보는 지시(STE-100, 랩터-3 「작업 중」).
//! 노드 에이전트는 사람이 스킬을 설치하지 않아도 이 스킬을 쓴다. 바뀌는 이유는 스킬 글과 그 배포 방식이다.
//! 근거: docs/design.md 「내장 스킬」.

use std::io;
use std::path::{Path, PathBuf};

use rust_embed::Embed;

#[derive(Embed)]
#[folder = "skills"]
struct Files;

/// Claude 플러그인 표지. 플러그인 스킬은 `loom:<스킬 이름>`으로 뜬다.
const MANIFEST: &str = r#"{"name":"loom","description":"loom이 싣는 스킬"}"#;

/// 데몬이 데이터 폴더에 푼 스킬 폴더. Claude는 이 폴더를 플러그인으로(`--plugin-dir`), Codex·OpenCode 2는
/// 그 안의 `skills/`를 스킬 폴더로 읽는다.
pub struct Plugin(PathBuf);

impl Plugin {
    /// `dir`을 지우고 이 판의 스킬로 다시 쓴다. 데몬이 시작할 때 한 번 부른다.
    /// 지우는 이유: 새 판에서 없앤 파일이 남으면 노드가 옛 글을 읽는다.
    /// 지우지 못하면(Windows에서 노드가 연 파일) 덮어 쓴다. 그때는 없앤 파일만 남는다.
    pub fn install(dir: PathBuf) -> io::Result<Self> {
        let _ = std::fs::remove_dir_all(&dir);
        let manifest = dir.join(".claude-plugin").join("plugin.json");
        write(&manifest, MANIFEST.as_bytes())?;
        let plugin = Self(dir);
        for name in Files::iter() {
            let file = Files::get(&name).expect("iter가 준 이름은 늘 있다");
            write(&plugin.skills().join(name.as_ref()), &file.data)?;
        }
        Ok(plugin)
    }

    pub fn dir(&self) -> &Path {
        &self.0
    }

    /// 스킬마다 `<이름>/SKILL.md`가 있는 폴더.
    pub fn skills(&self) -> PathBuf {
        self.0.join("skills")
    }
}

fn write(path: &Path, data: &[u8]) -> io::Result<()> {
    std::fs::create_dir_all(path.parent().expect("파일 경로에는 부모가 있다"))?;
    std::fs::write(path, data)
}

/// 모든 노드의 시작 지시에 붙이는 글: STE-100 본문과 랩터-3 「작업 중」 절.
/// 링크만 주면 모델이 읽지 않고 지나친다. 그래서 본문을 그대로 넣는다.
pub fn always_on() -> String {
    let ste = text("ste-100/SKILL.md");
    let ste = ste
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map_or(ste.as_str(), |(_, body)| body);
    let raptor = text("raptor-3/SKILL.md");
    let (_, rest) = raptor
        .split_once("\n## 작업 중\n")
        .expect("랩터-3에 「작업 중」 절이 있다");
    let section = rest.split("\n## ").next().unwrap_or(rest);
    format!("{}\n\n# 랩터-3 작업 중\n\n{}", ste.trim(), section.trim())
}

fn text(name: &str) -> String {
    let file = Files::get(name).expect("skills/ 폴더의 파일이다");
    String::from_utf8(file.data.into_owned()).expect("스킬 글은 UTF-8이다")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_on_has_the_ste_body_and_only_the_raptor_work_section() {
        let text = always_on();
        assert!(text.starts_with("# STE-100\n"), "{text}");
        assert!(!text.contains("name: ste-100"));
        assert!(text.contains("\n# 랩터-3 작업 중\n"));
        assert!(text.contains("완료 보고에는 `랩터-3:` 한 줄로"));
        assert!(!text.contains("## 화면(UI/UX) 감사"));
        assert!(!text.contains("## 5단계"));
    }

    #[test]
    fn install_writes_the_plugin_and_removes_old_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("plugin");
        write(&dir.join("skills/old/SKILL.md"), b"old").unwrap();
        let plugin = Plugin::install(dir.clone()).unwrap();
        assert!(!dir.join("skills/old").exists());
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join(".claude-plugin/plugin.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["name"], "loom");
        for skill in ["o-mode", "ste-100", "user-doc", "raptor-3"] {
            assert!(
                plugin.skills().join(skill).join("SKILL.md").is_file(),
                "{skill}"
            );
        }
        assert!(plugin.skills().join("o-mode/agent-rules.md").is_file());
        // 사람은 Orca와 loom 모두 `o-mode`로 부른다. loom 노드에서는 이 스킬이 그 이름을 받는다.
        let loom_mode = std::fs::read_to_string(plugin.skills().join("o-mode/SKILL.md")).unwrap();
        assert!(loom_mode.contains("\nname: o-mode\n"));
        assert!(plugin.skills().join("user-doc/scripts/show.sh").is_file());
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
