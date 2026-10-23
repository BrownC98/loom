//! 기능: 작업 폴더 `~/.cache/loom/<작업>/`. `loom spawn`이 만들고 자식에게 `LOOM_TASK_DIR`로 알린다.
//! 작업은 `--worktree` 브랜치다. 없으면 부모의 작업을 물려받는다. 근거: docs/design.md 「작업 폴더」.

use std::path::{Path, PathBuf};

use crate::wsl::Inside;

/// 노드 환경에서 작업 폴더를 가리키는 변수. 값은 노드가 보는 경로다(WSL 노드는 Linux 경로).
pub const ENV: &str = "LOOM_TASK_DIR";
pub const BRIEF: &str = "brief.md";
pub const RESULT: &str = "result.md";
/// 검토자가 쓰는 결함 파일 `review/<검토자 노드 이름>.md`의 폴더.
pub const REVIEW: &str = "review";
/// `loom merge`의 기록.
pub const LOGS: &str = "logs";

/// 브리프에 있어야 하는 머리. o-mode 브리프 틀과 같다.
pub const BRIEF_HEADS: [&str; 6] = ["Goal", "Facts", "Scope", "Done when", "Checks", "Time"];

/// `brief`에 없는 머리. 머리는 `<머리>:`로 시작하는 줄이거나 그 낱말의 markdown 제목(`## Goal`)이다. 대소문자는 가리지 않는다.
pub fn missing_heads(brief: &str) -> Vec<&'static str> {
    let heads: Vec<String> = brief.lines().filter_map(head).collect();
    BRIEF_HEADS
        .into_iter()
        .filter(|want| !heads.iter().any(|h| h.eq_ignore_ascii_case(want)))
        .collect()
}

/// 줄이 머리면 그 낱말.
fn head(line: &str) -> Option<String> {
    let line = line.trim();
    if let Some(title) = line.strip_prefix('#') {
        let title = title.trim_start_matches('#');
        return title
            .starts_with(' ')
            .then(|| title.trim().trim_end_matches(':').trim().to_owned());
    }
    line.split_once(':').map(|(word, _)| word.trim().to_owned())
}

/// 작업 이름의 폴더 이름. 한 단계 폴더라 `<작업 폴더>/..`가 모든 작업의 폴더다. 그래서 `/`가 든 브랜치(`feat/x`)는 `-`로 잇는다.
pub fn folder(task: &str) -> String {
    let name = task.trim().replace(['/', '\\', ':'], "-");
    // `.`, `..`는 위 폴더를 가리킨다.
    match name.trim_matches('.') {
        "" => name.replace('.', "-"),
        _ => name,
    }
}

/// 한 작업의 폴더: 데몬이 쓰는 경로와 노드가 보는 글자.
pub struct TaskDir {
    pub local: PathBuf,
    pub shown: String,
}

impl TaskDir {
    /// 노드가 도는 곳의 홈 아래. WSL 노드면 그 배포판의 홈이다(데몬은 UNC 경로로 쓴다).
    pub fn of(inside: Option<&Inside>, task: &str) -> Option<Self> {
        let name = folder(task);
        match inside {
            Some(inside) => {
                let shown = format!("{}/.cache/loom/{name}", inside.home());
                Some(Self {
                    local: inside.local(&shown),
                    shown,
                })
            }
            None => {
                let local = std::env::home_dir()?.join(".cache").join("loom").join(name);
                Some(Self {
                    shown: local.display().to_string(),
                    local,
                })
            }
        }
    }
}

/// `review/`, `logs/`, 빈 `result.md`를 만들고 `brief`가 있으면 `brief.md`에 쓴다. 이미 있는 파일은 두다.
/// 같은 작업의 자식(검토자 등)이 앞 브리프와 결과를 덮지 않게 한다.
pub fn create(dir: &Path, brief: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir.join(REVIEW))?;
    std::fs::create_dir_all(dir.join(LOGS))?;
    write_new(&dir.join(RESULT), "")?;
    if !brief.trim().is_empty() {
        write_new(&dir.join(BRIEF), brief)?;
    }
    Ok(())
}

fn write_new(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => file.write_all(text.as_bytes()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = "Goal: x\nFacts: y\n## Scope\n- a\nDone when: z\n### checks:\nTime: 2 h\n";

    #[test]
    fn heads_are_lines_with_a_colon_or_markdown_titles() {
        assert!(missing_heads(FULL).is_empty());
        assert_eq!(
            missing_heads("Goal: x\nTime: 1 h\nScope is src/"),
            vec!["Facts", "Scope", "Done when", "Checks"]
        );
        assert_eq!(missing_heads("#Goal\nGoals: x").len(), 6);
    }

    #[test]
    fn folder_is_one_level() {
        assert_eq!(folder("feat/x"), "feat-x");
        assert_eq!(folder(" 작업폴더 "), "작업폴더");
        assert_eq!(folder(".."), "--");
    }

    #[test]
    fn pc_dir_is_under_the_home_in_the_os_form() {
        let dir = TaskDir::of(None, "feat/x").unwrap();
        let want = std::env::home_dir()
            .unwrap()
            .join(".cache")
            .join("loom")
            .join("feat-x");
        assert_eq!(dir.local, want);
        assert_eq!(dir.shown, want.display().to_string());
        if cfg!(windows) {
            assert!(dir.shown.ends_with(r"\.cache\loom\feat-x"), "{}", dir.shown);
        }
    }

    #[test]
    fn create_keeps_earlier_files() {
        let dir = tempfile::tempdir().unwrap();
        let task = dir.path().join("t");
        create(&task, "first").unwrap();
        std::fs::write(task.join(RESULT), "done").unwrap();
        create(&task, "second").unwrap();
        assert_eq!(std::fs::read_to_string(task.join(BRIEF)).unwrap(), "first");
        assert_eq!(std::fs::read_to_string(task.join(RESULT)).unwrap(), "done");
        assert!(task.join(REVIEW).is_dir() && task.join(LOGS).is_dir());
        let empty = dir.path().join("e");
        create(&empty, " ").unwrap();
        assert!(!empty.join(BRIEF).exists());
    }
}
