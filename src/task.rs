use std::path::{Path, PathBuf};

pub const ENV: &str = "LOOM_TASK_DIR";
pub const BRIEF: &str = "brief.md";
pub const RESULT: &str = "result.md";
pub const REVIEW: &str = "review";
pub const LOGS: &str = "logs";

pub const BRIEF_HEADS: [&str; 6] = ["Goal", "Facts", "Scope", "Done when", "Checks", "Time"];

pub fn missing_heads(brief: &str) -> Vec<&'static str> {
    let heads: Vec<String> = brief.lines().filter_map(head).collect();
    BRIEF_HEADS
        .into_iter()
        .filter(|want| !heads.iter().any(|h| h.eq_ignore_ascii_case(want)))
        .collect()
}

fn head(line: &str) -> Option<String> {
    let line = line.trim();
    if let Some(title) = line.strip_prefix('#') {
        let title = title.trim_start_matches('#');
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

pub fn folder(base: &str) -> String {
    let name = base.trim().replace(['/', '\\', ':'], "-");
    match name.trim_matches('.') {
        "" => name.replace('.', "-"),
        _ => name,
    }
}

pub fn base(repo: &Path, name: &str) -> String {
    let repo = repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("{repo}-{name}")
}

pub const ROOT: [&str; 2] = [".cache", "loom"];

pub struct Tasks {
    local: PathBuf,
    shown: String,
    sep: char,
}

pub struct TaskDir {
    pub folder: String,
    pub local: PathBuf,
    pub shown: String,
}

impl Tasks {
    pub fn pc() -> Option<Self> {
        let local = ROOT
            .iter()
            .fold(std::env::home_dir()?, |path, part| path.join(part));
        Some(Self {
            shown: local.display().to_string(),
            local,
            sep: std::path::MAIN_SEPARATOR,
        })
    }

    pub fn wsl(home: &str, local: impl FnOnce(&str) -> PathBuf) -> Self {
        let shown = format!("{}/{}", home.trim_end_matches('/'), ROOT.join("/"));
        Self {
            local: local(&shown),
            shown,
            sep: '/',
        }
    }

    pub fn dir(&self, folder: &str) -> TaskDir {
        TaskDir {
            folder: folder.to_owned(),
            local: self.local.join(folder),
            shown: format!("{}{}{folder}", self.shown, self.sep),
        }
    }

    pub fn new_dir(&self, base: &str) -> std::io::Result<(TaskDir, Fresh)> {
        std::fs::create_dir_all(&self.local)?;
        let base = folder(base);
        for n in 1.. {
            let name = match n {
                1 => base.clone(),
                n => format!("{base}-{n}"),
            };
            match std::fs::create_dir(self.local.join(&name)) {
                Ok(()) => {
                    let dir = self.dir(&name);
                    let fresh = Fresh(Some(dir.local.clone()));
                    return Ok((dir, fresh));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        unreachable!("1.. does not end")
    }
}

pub struct Fresh(Option<PathBuf>);

impl Fresh {
    pub fn keep(mut self) {
        self.0 = None;
    }
}

impl Drop for Fresh {
    fn drop(&mut self) {
        if let Some(dir) = self.0.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

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
        let (first, keep_first) = tasks.new_dir("loom-feat/x").unwrap();
        keep_first.keep();
        create(&first.local, "old brief").unwrap();
        std::fs::write(first.local.join(REVIEW).join("opus.md"), "old").unwrap();
        let (again, keep_again) = tasks.new_dir("loom-feat/x").unwrap();
        let (other, keep_other) = tasks.new_dir("loom-feat-x").unwrap();
        keep_again.keep();
        keep_other.keep();
        assert_eq!(
            [&first.folder, &again.folder, &other.folder],
            ["loom-feat-x", "loom-feat-x-2", "loom-feat-x-3"]
        );
        assert_eq!(std::fs::read_dir(&again.local).unwrap().count(), 0);
        assert_eq!(
            again.shown,
            format!("{}/.cache/loom/loom-feat-x-2", home.path().display())
        );
        assert_eq!(tasks.dir(&first.folder).local, first.local);
    }

    #[test]
    fn a_dropped_fresh_folder_is_removed_and_a_kept_one_stays() {
        let home = tempfile::tempdir().unwrap();
        let tasks = Tasks::wsl(&home.path().display().to_string(), |p: &str| {
            PathBuf::from(p)
        });
        let (lost, fresh) = tasks.new_dir("loom-a").unwrap();
        create(&lost.local, "").unwrap();
        drop(fresh);
        assert!(!lost.local.exists());
        let (again, fresh) = tasks.new_dir("loom-a").unwrap();
        assert_eq!(again.folder, "loom-a");
        fresh.keep();
        assert!(again.local.exists());
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
