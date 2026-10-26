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

pub fn findings(branch: &str, reviewer: &str) -> String {
    format!("{REVIEW}/{}-{}.md", folder(branch), folder(reviewer))
}

pub fn findings_in_task(branch: &str, reviewer: &str) -> String {
    format!("${ENV}/{}", findings(branch, reviewer))
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
