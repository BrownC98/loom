pub mod merge;

use std::fmt;
use std::path::{Path, PathBuf};

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::db::Database;
use crate::distro::Ran;
use crate::git;
use crate::place::{self, Files, Machine, SshPath};

const LOOM_DIR: &str = "loom";
const WORKSPACES_DIR: &str = "workspaces";

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RepoId(pub u32);

#[derive(Serialize, TS, Clone)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RepoView {
    pub id: RepoId,
    pub name: String,
    /// 원래 체크아웃의 절대 경로.
    pub path: String,
    /// 첫째가 원래 체크아웃이다. git을 읽지 못하면 원래 체크아웃 하나만 있다.
    pub worktrees: Vec<Worktree>,
    /// 로컬 브랜치. [새 워크트리]의 기준 브랜치 목록이다.
    pub branches: Vec<String>,
    /// 거짓이면 첫 커밋 전이다(`git init` 직후). 워크트리를 만들 수 없다.
    pub has_commits: bool,
    /// 저장소가 도는 곳. 경로에서 나온 값이다.
    pub place: PlaceView,
    /// 화면에 보일 경로. WSL은 배포판 안의 경로이고, 홈 아래면 `~/…`다. SSH는 그 PC 안의 경로다.
    pub shown_path: String,
}

/// 폴더가 있는 곳: 이 PC, WSL 배포판, SSH 호스트. 근거: 목업 docs/mockups/wsl-place.html ②·②′, docs/mockups/ssh-repo.html ④.
#[derive(Serialize, TS, PartialEq, Debug, Clone)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum PlaceView {
    Pc,
    Wsl { distro: String },
    Ssh { host: String },
}

impl PlaceView {
    pub fn of(machine: &Machine) -> Self {
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
}

/// [저장소 추가] 위치 칩: 이 PC의 드라이브와 WSL 배포판. 배포판이 없으면 화면이 드라이브·배포판 칩을 보이지 않는다. [SSH] 칩은 늘 보인다.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct Locations {
    /// `C:\` 꼴. Windows 데몬이 아니면 비었다.
    pub drives: Vec<String>,
    pub distros: Vec<String>,
}

/// [저장소 추가]의 폴더 목록: 데몬이 보는 폴더 하나의 자식 폴더.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct FolderList {
    /// 목록을 읽은 폴더의 절대 경로.
    pub path: String,
    /// 맨 위 폴더면 없다.
    pub parent: Option<String>,
    /// 이름 순이다. 점으로 시작하는 폴더는 뺀다.
    pub folders: Vec<Folder>,
    /// 화면의 「실행 위치」 줄이 쓴다.
    pub place: PlaceView,
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct Folder {
    pub name: String,
    /// 안에 `.git`이 있다.
    pub git: bool,
}

#[derive(Serialize, TS, PartialEq, Debug, Clone)]
#[ts(export)]
pub struct Worktree {
    /// 절대 경로. 노드는 이 경로로 묶음을 가리킨다.
    pub path: String,
    /// 체크아웃한 커밋. git을 읽지 못하면 비어 있다.
    pub head: String,
    /// 분리된 HEAD면 없다.
    pub branch: Option<String>,
    /// 원래 체크아웃이면 참이다. 지울 수 없다.
    pub original: bool,
}

/// 저장소·워크트리 요청을 거절한 이유. 화면에는 코드와 값(JSON)이, 에이전트에게는 한국어 문장(`Display`)이 간다.
#[derive(Debug, PartialEq, Serialize, TS)]
#[serde(tag = "code", content = "value", rename_all = "camelCase")]
#[ts(export)]
pub enum RepoProblem {
    /// 데몬이 보는 파일 시스템에 이 폴더가 없다.
    NotFound(String),
    NotGit(String),
    /// 워크스페이스 폴더는 절대 경로다.
    NotAbsolute(String),
    /// 워크스페이스 폴더가 없다.
    NoWorkspace(String),
    /// 워크스페이스 폴더가 저장소와 다른 위치(이 PC, WSL 배포판)에 있다. WSL git은 Windows 경로에 워크트리를 만들지 못한다.
    OtherPlace(String),
    /// 없는 저장소 id다.
    NoRepo,
    /// 이 저장소의 워크트리가 아니다.
    NotInRepo(String),
    /// 원래 체크아웃은 지우지 않고 브랜치 이름도 바꾸지 않는다. git도 지우지 못한다.
    Original,
    /// 분리된 HEAD 워크트리다. 이름을 바꿀 브랜치가 없다.
    Detached,
    /// 이 브랜치의 `loom ready` 차례가 열려 있다. 차례가 끝난 뒤 이름을 바꾼다.
    OpenRound(String),
    EmptyBranch,
    /// 첫 커밋 전 저장소에서는 워크트리를 만들지 못한다.
    NoCommit,
    /// 새 저장소 폴더 이름으로 쓸 수 없다. 복제면 주소에서 얻은 이름이다.
    BadName(String),
    /// 복제할 폴더가 이미 있고 비어 있지 않다. 그 폴더는 건드리지 않는다.
    NotEmpty(String),
    /// 새 프로젝트 폴더가 이미 있다. 그 폴더는 건드리지 않는다.
    Exists(String),
    /// 이 저장소에 노드가 남았다. 값은 남은 노드 수다. 노드를 먼저 지운다.
    HasNodes(u32),
    /// git이 실패했다. 값은 git의 오류 글이다.
    Git(String),
}

impl fmt::Display for RepoProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RepoProblem::NotFound(path) => {
                write!(f, "{path}: not on the file system that the daemon sees.")
            }
            RepoProblem::NotGit(path) => write!(f, "{path}: not a git repository."),
            RepoProblem::NotAbsolute(path) => write!(f, "{path}: give an absolute path."),
            RepoProblem::NoWorkspace(path) => {
                write!(f, "{path}: the workspace folder does not exist.")
            }
            RepoProblem::OtherPlace(path) => write!(
                f,
                "{path}: the workspace folder is not in the same place (this PC or WSL distro) as the repository."
            ),
            RepoProblem::NoRepo => write!(f, "No such repository."),
            RepoProblem::NotInRepo(path) => write!(f, "{path}: not a worktree of this repository."),
            RepoProblem::Original => {
                write!(f, "The original checkout cannot be removed or renamed.")
            }
            RepoProblem::Detached => write!(f, "Detached HEAD. There is no branch to rename."),
            RepoProblem::OpenRound(branch) => write!(
                f,
                "{branch} has an open loom ready round. Rename it after the round ends."
            ),
            RepoProblem::EmptyBranch => write!(f, "Give a branch name."),
            RepoProblem::NoCommit => write!(
                f,
                "The repository has no commit yet. Commit first, then make a worktree."
            ),
            RepoProblem::BadName(name) => write!(f, "'{name}': cannot be a folder name."),
            RepoProblem::NotEmpty(path) => write!(f, "{path}: already exists and is not empty."),
            RepoProblem::Exists(path) => write!(f, "{path}: already exists."),
            RepoProblem::HasNodes(n) => {
                write!(
                    f,
                    "{n} nodes are still in this repository. Remove the nodes first."
                )
            }
            RepoProblem::Git(e) => write!(f, "git error: {e}"),
        }
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
) -> RepoView {
    let branches = branches(files, path).await;
    let worktrees = worktrees(files, path).await;
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
        place: PlaceView::of(&place::of(dir)),
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
    listed(files, repo).await.unwrap_or_else(|_| {
        vec![Worktree {
            path: repo.to_string_lossy().into_owned(),
            head: String::new(),
            branch: None,
            original: true,
        }]
    })
}

pub async fn listed(files: Files<'_>, repo: &Path) -> Result<Vec<Worktree>, String> {
    let porcelain = git::run(files, repo, &["worktree", "list", "--porcelain"]).await?;
    Ok(parse_worktrees(&porcelain)
        .into_iter()
        .map(|w| Worktree {
            path: git::from_git_path(repo, &w.path),
            ..w
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
