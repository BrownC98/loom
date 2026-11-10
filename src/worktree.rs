pub mod merge;

use std::path::{Path, PathBuf};

use rusqlite::OptionalExtension;

use api::ids::RepoId;
use api::repos::{Folder, FolderList, PlaceView, RepoProblem, RepoView, Worktree};

use crate::db::Database;
use crate::distro::Ran;
use crate::git;
use crate::place::{self, Files, Machine, SshPath};

const LOOM_DIR: &str = "loom";
const WORKSPACES_DIR: &str = "workspaces";

pub fn place_view(machine: &Machine) -> PlaceView {
    match machine {
        Machine::Pc => PlaceView::Pc,
        Machine::Wsl(at) => PlaceView::Wsl {
            distro: at.distro.clone(),
        },
        Machine::Ssh(at) => PlaceView::Ssh {
            host: at.host.clone(),
        },
    }
}

pub struct Repos {
    db: Database,
}

impl Repos {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    pub async fn add(&self, files: Files<'_>, path: &str) -> Result<RepoId, RepoProblem> {
        let folder = Path::new(path);
        check(files, folder).await?;
        let original = original_checkout(files, folder)
            .await
            .ok_or_else(|| RepoProblem::NotGit(path.to_owned()))?;
        self.insert(&original)
            .map_err(|e| RepoProblem::Git(e.to_string()))
    }

    pub async fn clone(
        &self,
        files: Files<'_>,
        url: &str,
        parent: &str,
    ) -> Result<RepoId, RepoProblem> {
        let url = url.trim();
        let target = new_folder(parent, repo_name(url))?;
        if occupied(files, &target).await.map_err(RepoProblem::Git)? {
            return Err(RepoProblem::NotEmpty(target.to_string_lossy().into_owned()));
        }
        git::clone(files, url, &target)
            .await
            .map_err(RepoProblem::Git)?;
        self.add(files, &target.to_string_lossy()).await
    }

    pub async fn create(
        &self,
        files: Files<'_>,
        parent: &str,
        name: &str,
    ) -> Result<RepoId, RepoProblem> {
        let target = new_folder(parent, name.trim())?;
        let text = target.to_string_lossy().into_owned();
        if exists(files, &target).await.map_err(RepoProblem::Git)? {
            return Err(RepoProblem::Exists(text));
        }
        files
            .create_dir_all(&target)
            .map_err(|e| RepoProblem::Git(e.to_string()))?;
        if let Err(e) = git::run(files, &target, &["init", "-q"]).await {
            let _ = remove_dir(files, &target).await;
            return Err(RepoProblem::Git(e));
        }
        self.add(files, &text).await
    }

    fn insert(&self, path: &Path) -> rusqlite::Result<RepoId> {
        let text = path.to_string_lossy();
        let db = self.db.lock();
        db.execute(
            "INSERT INTO repos (path) VALUES (?1) ON CONFLICT (path) DO NOTHING",
            [&text],
        )?;
        db.query_row("SELECT id FROM repos WHERE path = ?1", [&text], |row| {
            Ok(RepoId(row.get(0)?))
        })
    }

    pub fn list(&self) -> rusqlite::Result<Vec<(RepoId, PathBuf)>> {
        self.db
            .lock()
            .prepare("SELECT id, path FROM repos ORDER BY id")?
            .query_map([], |row| {
                Ok((RepoId(row.get(0)?), PathBuf::from(row.get::<_, String>(1)?)))
            })?
            .collect()
    }

    pub fn remove(&self, id: RepoId) -> Result<(), RepoProblem> {
        let db = self.db.lock();
        let fail = |e: rusqlite::Error| RepoProblem::Git(e.to_string());
        let deleted = db
            .execute(
                "DELETE FROM repos WHERE id = ?1 AND NOT EXISTS (SELECT 1 FROM nodes WHERE repo = ?1)",
                [id.0],
            )
            .map_err(fail)?;
        if deleted > 0 {
            return Ok(());
        }
        let left: Option<u32> = db
            .query_row(
                "SELECT (SELECT count(*) FROM nodes WHERE repo = ?1) FROM repos WHERE id = ?1",
                [id.0],
                |row| row.get(0),
            )
            .optional()
            .map_err(fail)?;
        Err(left.map_or(RepoProblem::NoRepo, RepoProblem::HasNodes))
    }

    pub fn path(&self, id: RepoId) -> Result<PathBuf, RepoProblem> {
        self.db
            .lock()
            .query_row("SELECT path FROM repos WHERE id = ?1", [id.0], |row| {
                row.get::<_, String>(0)
            })
            .optional()
            .map_err(|e| RepoProblem::Git(e.to_string()))?
            .map(PathBuf::from)
            .ok_or(RepoProblem::NoRepo)
    }
}

pub async fn check(files: Files<'_>, path: &Path) -> Result<(), RepoProblem> {
    let text = || path.to_string_lossy().into_owned();
    if !is_dir(files, path).await.map_err(RepoProblem::Git)? {
        return Err(RepoProblem::NotFound(text()));
    }
    git::run(files, path, &["rev-parse", "--git-dir"])
        .await
        .map(drop)
        .map_err(|_| RepoProblem::NotGit(text()))
}

pub async fn branches(files: Files<'_>, dir: &Path) -> Vec<String> {
    git::run(files, dir, &["branch", "--format=%(refname:short)"])
        .await
        .map(|out| out.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

pub async fn view(
    files: Files<'_>,
    id: RepoId,
    path: &Path,
    place: PlaceView,
    shown_path: String,
    worktrees: Vec<Worktree>,
) -> RepoView {
    let branches = branches(files, path).await;
    RepoView {
        id,
        name: folder_name(path),
        path: path.to_string_lossy().into_owned(),
        has_commits: has_commits(&worktrees),
        worktrees,
        branches,
        place,
        shown_path,
    }
}

fn has_commits(worktrees: &[Worktree]) -> bool {
    worktrees
        .iter()
        .find(|w| w.original)
        .is_some_and(|w| w.head.bytes().any(|b| b != b'0'))
}

pub fn folders(files: Files, path: &str) -> Result<FolderList, RepoProblem> {
    let dir = Path::new(path);
    if !place::absolute(dir) {
        return Err(RepoProblem::NotAbsolute(path.to_owned()));
    }
    let entries = files.list(dir).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => {
            RepoProblem::NotFound(path.to_owned())
        }
        _ => RepoProblem::Git(e.to_string()),
    })?;
    let mut folders: Vec<Folder> = entries
        .into_iter()
        .filter(|entry| entry.dir && !entry.name.starts_with('.'))
        .map(|entry| Folder {
            git: files
                .list(&dir.join(&entry.name))
                .is_ok_and(|inside| inside.iter().any(|e| e.name == ".git")),
            name: entry.name,
        })
        .collect();
    folders.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(FolderList {
        path: path.to_owned(),
        parent: place::parent(dir),
        folders,
        place: place_view(&place::of(dir)),
    })
}

pub fn drives() -> Vec<String> {
    #[cfg(windows)]
    {
        unsafe extern "system" {
            fn GetLogicalDrives() -> u32;
        }
        let mask = unsafe { GetLogicalDrives() };
        (b'A'..=b'Z')
            .filter(|letter| mask & (1 << (letter - b'A')) != 0)
            .map(|letter| format!("{}:\\", letter as char))
            .collect()
    }
    #[cfg(not(windows))]
    Vec::new()
}

fn new_folder(parent: &str, name: &str) -> Result<PathBuf, RepoProblem> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err(RepoProblem::BadName(name.to_owned()));
    }
    let dir = Path::new(parent);
    if !place::absolute(dir) {
        return Err(RepoProblem::NotAbsolute(parent.to_owned()));
    }
    Ok(dir.join(name))
}

fn repo_name(url: &str) -> &str {
    let url = url.trim_end_matches('/');
    let leaf = url.rsplit(['/', ':']).next().unwrap_or_default();
    leaf.strip_suffix(".git").unwrap_or(leaf)
}

pub async fn worktrees(files: Files<'_>, repo: &Path) -> Vec<Worktree> {
    listed(files, repo).await.unwrap_or_else(|_| alone(repo))
}

pub fn alone(repo: &Path) -> Vec<Worktree> {
    vec![Worktree {
        path: repo.to_string_lossy().into_owned(),
        head: String::new(),
        branch: None,
        original: true,
    }]
}

pub async fn listed(files: Files<'_>, repo: &Path) -> Result<Vec<Worktree>, String> {
    let porcelain = git::run(files, repo, &["worktree", "list", "--porcelain"]).await?;
    Ok(parse_worktrees(&porcelain)
        .into_iter()
        .map(|w| Worktree {
            path: git::from_git_path(repo, &w.path),
            ..w
        })
        .filter(|w| {
            Path::new(&w.path).parent().and_then(Path::file_name)
                != Some(std::ffi::OsStr::new(merge::BATCHES))
        })
        .collect())
}

fn parse_worktrees(porcelain: &str) -> Vec<Worktree> {
    porcelain
        .split("\n\n")
        .filter_map(|block| {
            let field = |key: &str| {
                block
                    .lines()
                    .find_map(|line| line.strip_prefix(key)?.strip_prefix(' '))
            };
            Some(Worktree {
                path: field("worktree")?.to_owned(),
                head: field("HEAD")?.to_owned(),
                branch: field("branch")
                    .map(|b| b.strip_prefix("refs/heads/").unwrap_or(b).to_owned()),
                original: false,
            })
        })
        .enumerate()
        .map(|(i, w)| Worktree {
            original: i == 0,
            ..w
        })
        .collect()
}

pub async fn find(files: Files<'_>, repo: &Path, path: &str) -> Result<Worktree, RepoProblem> {
    listed(files, repo)
        .await
        .map_err(RepoProblem::Git)?
        .into_iter()
        .find(|w| w.path == path)
        .ok_or_else(|| RepoProblem::NotInRepo(path.to_owned()))
}

pub async fn top_of(files: Files<'_>, cwd: &Path) -> Result<String, String> {
    let top = git::run(files, cwd, &["rev-parse", "--show-toplevel"]).await?;
    Ok(git::from_git_path(cwd, top.trim()))
}

pub async fn create(
    files: Files<'_>,
    repo: &Path,
    from: &Path,
    workspace: &Path,
    branch: &str,
    base: &str,
    unowned: impl Fn(&str) -> bool,
) -> Result<String, RepoProblem> {
    let branch = branch.trim();
    let leaf = branch.rsplit('/').next().unwrap_or_default();
    if leaf.is_empty() {
        return Err(RepoProblem::EmptyBranch);
    }
    if !has_commits(&listed(files, repo).await.map_err(RepoProblem::Git)?) {
        return Err(RepoProblem::NoCommit);
    }
    if !place::absolute(workspace) {
        return Err(RepoProblem::NotAbsolute(
            workspace.to_string_lossy().into_owned(),
        ));
    }
    if !place::of(repo).same(&place::of(workspace)) {
        return Err(RepoProblem::OtherPlace(
            workspace.to_string_lossy().into_owned(),
        ));
    }
    if !is_dir(files, workspace).await.map_err(RepoProblem::Git)? {
        return Err(RepoProblem::NoWorkspace(
            workspace.to_string_lossy().into_owned(),
        ));
    }
    let parent = workspace.join(folder_name(repo));
    files
        .create_dir_all(&parent)
        .map_err(|e| RepoProblem::Git(e.to_string()))?;
    let path = parent.join(leaf).to_string_lossy().into_owned();
    let target = git::to_git_path(from, &path);
    if let Err(e) = git::run(
        files,
        from,
        &["worktree", "add", "-b", branch, &target, base],
    )
    .await
    {
        return left_over(files, repo, from, &path, &target, branch, base, unowned)
            .await
            .ok_or(RepoProblem::Git(e));
    }
    Ok(listed_path(files, repo, &path, &target).await)
}

#[expect(clippy::too_many_arguments)]
async fn left_over(
    files: Files<'_>,
    repo: &Path,
    from: &Path,
    path: &str,
    target: &str,
    branch: &str,
    base: &str,
    unowned: impl Fn(&str) -> bool,
) -> Option<String> {
    let base = git::run(files, from, &["rev-parse", "--verify", "--quiet", base])
        .await
        .ok()?;
    let tip = format!("refs/heads/{branch}");
    let tip = git::run(files, from, &["rev-parse", "--verify", "--quiet", &tip])
        .await
        .ok()?;
    let base = base.trim().to_owned();
    if tip.trim() != base {
        return None;
    }
    let listed = worktrees(files, repo).await;
    match listed.iter().find(|w| w.branch.as_deref() == Some(branch)) {
        Some(made) => {
            let same = real(files, &made.path).await == real(files, path).await;
            let clean = !dirty(files, Path::new(&made.path)).await.ok()?;
            (same && clean && made.head == base && unowned(&made.path)).then(|| made.path.clone())
        }
        None => {
            git::run(files, from, &["worktree", "add", target, branch])
                .await
                .ok()?;
            Some(listed_path(files, repo, path, target).await)
        }
    }
}

async fn listed_path(files: Files<'_>, repo: &Path, path: &str, target: &str) -> String {
    if let Some(made) = real(files, path).await {
        for listed in worktrees(files, repo).await {
            if real(files, &listed.path).await.as_ref() == Some(&made) {
                return listed.path;
            }
        }
    }
    git::from_git_path(repo, target)
}

pub async fn changes(files: Files<'_>, path: &Path) -> Result<Vec<String>, RepoProblem> {
    if !is_dir(files, path).await.map_err(RepoProblem::Git)? {
        return Ok(Vec::new());
    }
    let status = git::run(
        files,
        path,
        &["status", "--porcelain", "--untracked-files=normal"],
    )
    .await
    .map_err(RepoProblem::Git)?;
    Ok(status.lines().map(str::to_owned).collect())
}

pub async fn dirty(files: Files<'_>, path: &Path) -> Result<bool, RepoProblem> {
    changes(files, path).await.map(|listed| !listed.is_empty())
}

pub async fn remove(files: Files<'_>, repo: &Path, worktree: &Worktree) -> Result<(), RepoProblem> {
    git_remove(files, repo, worktree, &["--force"]).await
}

pub async fn remove_clean(
    files: Files<'_>,
    repo: &Path,
    worktree: &Worktree,
) -> Result<(), RepoProblem> {
    git_remove(files, repo, worktree, &[]).await
}

async fn git_remove(
    files: Files<'_>,
    repo: &Path,
    worktree: &Worktree,
    extra: &[&str],
) -> Result<(), RepoProblem> {
    if worktree.original {
        return Err(RepoProblem::Original);
    }
    let target = git::to_git_path(repo, &worktree.path);
    let mut args = vec!["worktree", "remove"];
    args.extend_from_slice(extra);
    args.push(&target);
    git::run(files, repo, &args)
        .await
        .map_err(RepoProblem::Git)?;
    Ok(())
}

pub async fn rename(
    files: Files<'_>,
    repo: &Path,
    worktree: &Worktree,
    name: &str,
) -> Result<(), RepoProblem> {
    if worktree.original {
        return Err(RepoProblem::Original);
    }
    let Some(branch) = &worktree.branch else {
        return Err(RepoProblem::Detached);
    };
    let name = name.trim();
    if name.is_empty() {
        return Err(RepoProblem::EmptyBranch);
    }
    git::run(files, repo, &["branch", "-m", "--", branch, name])
        .await
        .map_err(RepoProblem::Git)?;
    Ok(())
}

pub async fn delete_branch(files: Files<'_>, repo: &Path, branch: &str) -> Result<(), RepoProblem> {
    git::run(files, repo, &["branch", "-D", "--", branch])
        .await
        .map_err(RepoProblem::Git)?;
    Ok(())
}

async fn original_checkout(files: Files<'_>, folder: &Path) -> Option<PathBuf> {
    git::run(files, folder, &["rev-parse", "--show-toplevel"])
        .await
        .ok()?;
    let first = worktrees(files, folder).await.into_iter().next()?;
    Some(PathBuf::from(first.path))
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub async fn is_dir(files: Files<'_>, path: &Path) -> Result<bool, String> {
    test(files, path, Path::is_dir, r#"[ -d "$1" ]"#).await
}

pub async fn is_file(files: Files<'_>, path: &Path) -> Result<bool, String> {
    test(files, path, Path::is_file, r#"[ -f "$1" ]"#).await
}

async fn exists(files: Files<'_>, path: &Path) -> Result<bool, String> {
    test(files, path, Path::exists, r#"[ -e "$1" ]"#).await
}

async fn occupied(files: Files<'_>, path: &Path) -> Result<bool, String> {
    let here =
        |path: &Path| std::fs::read_dir(path).map_or(path.exists(), |mut d| d.next().is_some());
    let there = r#"[ -e "$1" ] && { [ ! -d "$1" ] || [ -n "$(ls -A "$1")" ]; }"#;
    test(files, path, here, there).await
}

async fn remove_dir(files: Files<'_>, path: &Path) -> Result<bool, String> {
    let here = |path: &Path| std::fs::remove_dir(path).is_ok();
    test(files, path, here, r#"rmdir "$1""#).await
}

async fn real(files: Files<'_>, path: &str) -> Option<PathBuf> {
    match place::parse(path) {
        Machine::Pc | Machine::Wsl(_) => Path::new(path).canonicalize().ok(),
        Machine::Ssh(at) => {
            let found = shell(files, &at, r#"cd "$1" && pwd -P"#).await.ok()?;
            (found.code == Some(0)).then(|| at.url(found.stdout.trim()).into())
        }
    }
}

async fn test(
    files: Files<'_>,
    path: &Path,
    here: impl FnOnce(&Path) -> bool,
    there: &str,
) -> Result<bool, String> {
    match place::of(path) {
        Machine::Pc | Machine::Wsl(_) => Ok(here(path)),
        Machine::Ssh(at) => Ok(shell(files, &at, there).await?.code == Some(0)),
    }
}

async fn shell(files: Files<'_>, at: &SshPath, script: &str) -> Result<Ran, String> {
    let argv = ["sh", "-c", script, "sh", &at.path];
    files
        .run(&at.host, git::remote(argv, &[]), place::RUN_WAIT)
        .await
}

pub fn setup_script(files: Files, worktree: &Path) -> Option<String> {
    let text = String::from_utf8(files.read(&worktree.join("orca.yaml")).ok()?).ok()?;
    let docs = yaml_rust2::YamlLoader::load_from_str(&text).ok()?;
    let script = docs.first()?["scripts"]["setup"].as_str()?;
    (!script.trim().is_empty()).then(|| script.to_owned())
}

pub fn default_workspace() -> std::io::Result<PathBuf> {
    workspace_in(&loom_home()?)
}

pub fn workspace_under(home: &str) -> String {
    format!("{home}/{LOOM_DIR}/{WORKSPACES_DIR}")
}

pub fn workspace_in(loom: &Path) -> std::io::Result<PathBuf> {
    let dir = loom.join(WORKSPACES_DIR);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn loom_home() -> std::io::Result<PathBuf> {
    let home =
        std::env::home_dir().ok_or_else(|| std::io::Error::other("홈 폴더를 찾지 못했다"))?;
    let dir = home.join(LOOM_DIR);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}
