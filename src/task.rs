use std::path::{Path, PathBuf};

use crate::place::Files;

pub const ENV: &str = "LOOM_TASK_DIR";
pub const BRIEF: &str = "brief.md";
pub const RESULT: &str = "result.md";
pub const REVIEW: &str = "review";
pub const LOGS: &str = "logs";

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

const OWNER: &str = ".loom-owner";

pub const HIDE: &str = r#"set -u
mins=$1 root=$2
shift 2
for t in "$root"/.tidy-*; do
  [ -d "$t" ] || continue
  d=$root/${t##*/.tidy-}
  [ -e "$d" ] || mv -- "$t" "$d"
done
for name in "$@"; do
  d=$root/$name
  [ -d "$d" ] && [ ! -L "$d" ] || continue
  recent=$(find "$d" -mmin -"$mins" -print -quit 2>/dev/null) || continue
  [ -z "$recent" ] || continue
  mv -- "$d" "$root/.tidy-$name" && printf '%s\n' "$name"
done
"#;

pub const DROP: &str = r#"set -u
root=$1
shift
while [ $# -gt 0 ] && [ "$1" != -- ]; do
  [ -e "$root/$1" ] || mv -- "$root/.tidy-$1" "$root/$1"
  shift
done
[ $# -gt 0 ] && shift
n=0 kib=0
for name in "$@"; do
  t=$root/.tidy-$name
  [ -d "$t" ] || continue
  k=$(du -sk "$t" | cut -f1)
  rm -rf -- "$t" && n=$((n + 1)) kib=$((kib + k))
done
echo "$n $kib"
"#;

pub struct Tasks<'a> {
    local: PathBuf,
    shown: String,
    sep: char,
    files: Files<'a>,
}

pub struct TaskDir {
    pub folder: String,
    pub local: PathBuf,
    pub shown: String,
}

impl<'a> Tasks<'a> {
    pub fn pc(files: Files<'a>) -> Option<Self> {
        let local = ROOT
            .iter()
            .fold(std::env::home_dir()?, |path, part| path.join(part));
        Some(Self {
            shown: local.display().to_string(),
            local,
            sep: std::path::MAIN_SEPARATOR,
            files,
        })
    }

    pub fn inside(home: &str, local: impl FnOnce(&str) -> PathBuf, files: Files<'a>) -> Self {
        let shown = format!("{}/{}", home.trim_end_matches('/'), ROOT.join("/"));
        Self {
            local: local(&shown),
            shown,
            sep: '/',
            files,
        }
    }

    pub fn dir(&self, folder: &str) -> TaskDir {
        TaskDir {
            folder: folder.to_owned(),
            local: self.local.join(folder),
            shown: format!("{}{}{folder}", self.shown, self.sep),
        }
    }

    pub fn shown(&self) -> &str {
        &self.shown
    }

    pub fn owned_by(&self, owner: &str) -> std::io::Result<bool> {
        self.files.create_dir_all(&self.local)?;
        let marker = self.local.join(OWNER);
        match self.files.create_new(&marker, owner) {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                Ok(String::from_utf8_lossy(&self.files.read(&marker)?).trim() == owner)
            }
            made => made.map(|()| true),
        }
    }

    pub fn of_repo(&self, repo: &Path) -> std::io::Result<Vec<TaskDir>> {
        let prefix = base(repo, "");
        let listed = match self.files.list(&self.local) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            listed => listed?,
        };
        Ok(listed
            .into_iter()
            .filter(|entry| entry.dir && entry.name.starts_with(&prefix))
            .map(|entry| self.dir(&entry.name))
            .collect())
    }

    pub fn new_dir(&self, base: &str) -> std::io::Result<(TaskDir, Fresh<'a>)> {
        self.files.create_dir_all(&self.local)?;
        let base = folder(base);
        for n in 1.. {
            let name = match n {
                1 => base.clone(),
                n => format!("{base}-{n}"),
            };
            match self.files.create_dir(&self.local.join(&name)) {
                Ok(()) => {
                    let dir = self.dir(&name);
                    let fresh = Fresh(Some((dir.local.clone(), self.files)));
                    return Ok((dir, fresh));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        unreachable!("1.. does not end")
    }
}

pub fn create(files: Files<'_>, dir: &Path, brief: &str) -> std::io::Result<()> {
    files.create_dir_all(&dir.join(REVIEW))?;
    files.create_dir_all(&dir.join(LOGS))?;
    write_new(files, &dir.join(RESULT), "")?;
    if !brief.trim().is_empty() {
        write_new(files, &dir.join(BRIEF), brief)?;
    }
    Ok(())
}

fn write_new(files: Files<'_>, path: &Path, text: &str) -> std::io::Result<()> {
    match files.create_new(path, text) {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        done => done,
    }
}

pub struct Fresh<'a>(Option<(PathBuf, Files<'a>)>);

impl Fresh<'_> {
    pub fn keep(mut self) {
        self.0 = None;
    }
}

impl Drop for Fresh<'_> {
    fn drop(&mut self) {
        if let Some((dir, files)) = self.0.take() {
            let _ = files.remove_dir_all(&dir);
        }
    }
}
