//! 기능: 작업 폴더 `~/.cache/loom/<작업>/`. `loom spawn`과 사람이 띄운 노드가 만들고 노드에게 `LOOM_TASK_DIR`로 알린다.
//! 폴더 이름은 데몬이 처음 한 번 정해 노드 줄(`nodes.task`)에 둔다. 근거: docs/design.md 「작업 폴더」.

use std::path::{Path, PathBuf};

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

/// `brief`에 없는 머리. 머리는 `<머리>:`로 시작하는 줄이거나 그 낱말의 ATX 제목(`## Goal`, `## Goal ##`)이다. 대소문자는 가리지 않는다.
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
        // ATX: 여는 `#` 뒤에 빈칸이나 탭, 끝의 닫는 `#`는 제목 글이 아니다.
        return title.starts_with([' ', '\t']).then(|| {
            title
                .trim()
                .trim_end_matches('#')
                .trim()
                .trim_end_matches(':')
                .trim()
                .to_owned()
        });
    }
    line.split_once(':').map(|(word, _)| word.trim().to_owned())
}

/// 폴더 이름으로 쓸 글. 한 단계 폴더다: `/`가 든 브랜치(`feat/x`)는 `-`로 잇는다. 같은 이름의 다른 작업은 `Tasks::new_dir`가 번호로 가른다.
pub fn folder(base: &str) -> String {
    let name = base.trim().replace(['/', '\\', ':'], "-");
    // `.`, `..`는 위 폴더를 가리킨다.
    match name.trim_matches('.') {
        "" => name.replace('.', "-"),
        _ => name,
    }
}

/// 새 작업의 바탕 이름: `<저장소 폴더 이름>-<브랜치 또는 노드 이름>`.
pub fn base(repo: &Path, name: &str) -> String {
    let repo = repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("{repo}-{name}")
}

/// 노드의 작업.
pub enum Pick {
    /// 새 작업. 값은 바탕 이름(`base`)이다.
    New(String),
    /// 이미 정한 작업 폴더 이름(`nodes.task`). 부모의 작업을 물려받을 때와 다시 띄울 때다.
    Joined(String),
}

/// 모든 작업 폴더가 든 폴더 `~/.cache/loom`. 노드가 도는 곳의 홈 아래다.
pub struct Tasks {
    local: PathBuf,
    shown: String,
    /// 노드가 보는 경로의 구분 글자. WSL 노드는 `/`다.
    sep: char,
}

/// 한 작업의 폴더.
pub struct TaskDir {
    /// `nodes.task`에 두는 이름.
    pub folder: String,
    /// 데몬과 CLI가 쓰는 경로(WSL 폴더면 UNC 경로).
    pub local: PathBuf,
    /// 노드가 보는 경로(`LOOM_TASK_DIR`).
    pub shown: String,
}

impl Tasks {
    /// 이 PC(Linux·macOS·Windows 노드).
    pub fn pc() -> Option<Self> {
        let local = std::env::home_dir()?.join(".cache").join("loom");
        Some(Self {
            shown: local.display().to_string(),
            local,
            sep: std::path::MAIN_SEPARATOR,
        })
    }

    /// WSL 노드. `home`은 배포판 안의 홈, `local`은 배포판 안의 경로를 데몬이 쓰는 UNC 경로로 바꾼다.
    pub fn wsl(home: &str, local: impl FnOnce(&str) -> PathBuf) -> Self {
        let shown = format!("{}/.cache/loom", home.trim_end_matches('/'));
        Self {
            local: local(&shown),
            shown,
            sep: '/',
        }
    }

    /// 이미 정한 작업 폴더. 폴더는 `create`가 만든다.
    pub fn dir(&self, folder: &str) -> TaskDir {
        TaskDir {
            folder: folder.to_owned(),
            local: self.local.join(folder),
            shown: format!("{}{}{folder}", self.shown, self.sep),
        }
    }

    /// 새 작업 폴더를 만든다. 그 이름의 폴더가 이미 있으면(앞 작업, 다른 브랜치가 같은 이름이 됨) `-2`, `-3`…을 붙인다.
    /// `create_dir`은 하나만 성공하므로 동시에 띄운 두 자식도 다른 폴더를 받는다.
    pub fn new_dir(&self, base: &str) -> std::io::Result<TaskDir> {
        std::fs::create_dir_all(&self.local)?;
        let base = folder(base);
        for n in 1.. {
            let name = match n {
                1 => base.clone(),
                n => format!("{base}-{n}"),
            };
            match std::fs::create_dir(self.local.join(&name)) {
                Ok(()) => return Ok(self.dir(&name)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        unreachable!("1.. does not end")
    }

    pub fn open(&self, pick: &Pick) -> std::io::Result<TaskDir> {
        match pick {
            Pick::New(base) => self.new_dir(base),
            Pick::Joined(folder) => Ok(self.dir(folder)),
        }
    }
}

/// `review/`, `logs/`, 빈 `result.md`를 만들고 `brief`가 있으면 `brief.md`에 쓴다. 이미 있는 파일은 둔다.
/// 같은 작업을 물려받은 자식(검토자 등)이 앞 브리프와 결과를 덮지 않게 한다. 새 작업은 늘 빈 새 폴더다(`Tasks::new_dir`).
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
    fn heads_are_lines_with_a_colon_or_atx_titles() {
        assert!(missing_heads(FULL).is_empty());
        assert_eq!(
            missing_heads("Goal: x\nTime: 1 h\nScope is src/"),
            vec!["Facts", "Scope", "Done when", "Checks"]
        );
        assert_eq!(missing_heads("#Goal\nGoals: x").len(), 6);
        let closed =
            "## Goal ##\n##\tFacts\n# Scope #\n### Done when ###\n##\tChecks\t##\n## Time:\n";
        assert!(
            missing_heads(closed).is_empty(),
            "{:?}",
            missing_heads(closed)
        );
    }

    #[test]
    fn folder_is_one_level() {
        assert_eq!(folder("feat/x"), "feat-x");
        assert_eq!(folder(" 작업폴더 "), "작업폴더");
        assert_eq!(folder(".."), "--");
        assert_eq!(base(Path::new("/src/loom"), "feat/x"), "loom-feat/x");
    }

    #[test]
    fn pc_dir_is_under_the_home_in_the_os_form() {
        let dir = Tasks::pc().unwrap().dir("loom-feat-x");
        let want = std::env::home_dir()
            .unwrap()
            .join(".cache")
            .join("loom")
            .join("loom-feat-x");
        assert_eq!(dir.local, want);
        assert_eq!(dir.shown, want.display().to_string());
        if cfg!(windows) {
            assert!(
                dir.shown.ends_with(r"\.cache\loom\loom-feat-x"),
                "{}",
                dir.shown
            );
        }
    }

    #[test]
    fn a_reused_or_colliding_name_gets_a_new_empty_folder() {
        let home = tempfile::tempdir().unwrap();
        let tasks = Tasks::wsl(&home.path().display().to_string(), |p: &str| {
            PathBuf::from(p)
        });
        let first = tasks.new_dir("loom-feat/x").unwrap();
        create(&first.local, "old brief").unwrap();
        std::fs::write(first.local.join(REVIEW).join("opus.md"), "old").unwrap();
        // 같은 브랜치를 병합 뒤 다시 쓴다. `feat-x`는 다른 브랜치지만 같은 글이 된다.
        let again = tasks.new_dir("loom-feat/x").unwrap();
        let other = tasks.new_dir("loom-feat-x").unwrap();
        assert_eq!(
            [&first.folder, &again.folder, &other.folder],
            ["loom-feat-x", "loom-feat-x-2", "loom-feat-x-3"]
        );
        assert_eq!(std::fs::read_dir(&again.local).unwrap().count(), 0);
        assert_eq!(
            again.shown,
            format!("{}/.cache/loom/loom-feat-x-2", home.path().display())
        );
        let joined = tasks.open(&Pick::Joined(first.folder.clone())).unwrap();
        assert_eq!(joined.local, first.local);
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
