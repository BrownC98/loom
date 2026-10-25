pub mod merge;

use std::fmt;
use std::path::{Path, PathBuf};

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::db::Database;
use crate::git;
use crate::wsl;

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
    /// WSL 폴더의 저장소면 그 배포판이다. 이 PC 저장소와 다른 OS의 데몬에서는 없다. 경로에서 나온 값이다.
    pub wsl: Option<WslPlace>,
}

/// 저장소가 도는 WSL 배포판. 근거: 목업 docs/mockups/wsl-place.html ②·②′.
#[derive(Serialize, TS, PartialEq, Debug, Clone)]
#[ts(export)]
pub struct WslPlace {
    pub distro: String,
    /// 화면에 보일 배포판 안의 경로. 홈 아래면 `~/…`다.
    pub path: String,
}

/// [저장소 추가] 위치 칩: 이 PC의 드라이브와 WSL 배포판. 배포판이 없으면 화면이 칩을 보이지 않는다.
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
    /// WSL 폴더면 그 배포판이다. 화면의 「실행 위치」 줄이 쓴다.
    pub distro: Option<String>,
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

    pub async fn add(&self, path: &str) -> Result<RepoId, RepoProblem> {
        let folder = Path::new(path);
        check(folder).await?;
        let original = original_checkout(folder)
            .await
            .ok_or_else(|| RepoProblem::NotGit(path.to_owned()))?;
        self.insert(&original)
            .map_err(|e| RepoProblem::Git(e.to_string()))
    }

    pub async fn clone(&self, url: &str, parent: &str) -> Result<RepoId, RepoProblem> {
        let url = url.trim();
        let target = new_folder(parent, repo_name(url))?;
        let occupied =
            std::fs::read_dir(&target).map_or(target.exists(), |mut d| d.next().is_some());
        if occupied {
            return Err(RepoProblem::NotEmpty(target.to_string_lossy().into_owned()));
        }
        git::clone(url, &target).await.map_err(RepoProblem::Git)?;
        self.add(&target.to_string_lossy()).await
    }

    pub async fn create(&self, parent: &str, name: &str) -> Result<RepoId, RepoProblem> {
        let target = new_folder(parent, name.trim())?;
        let text = target.to_string_lossy().into_owned();
        if target.exists() {
            return Err(RepoProblem::Exists(text));
        }
        std::fs::create_dir_all(&target).map_err(|e| RepoProblem::Git(e.to_string()))?;
        if let Err(e) = git::run(&target, &["init", "-q"]).await {
            let _ = std::fs::remove_dir(&target);
            return Err(RepoProblem::Git(e));
        }
        self.add(&text).await
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

pub async fn check(path: &Path) -> Result<(), RepoProblem> {
    let text = || path.to_string_lossy().into_owned();
    if !path.is_dir() {
        return Err(RepoProblem::NotFound(text()));
    }
    git::run(path, &["rev-parse", "--git-dir"])
        .await
        .map(drop)
        .map_err(|_| RepoProblem::NotGit(text()))
}

pub async fn view(id: RepoId, path: &Path, wsl: Option<WslPlace>) -> RepoView {
    let branches = git::run(path, &["branch", "--format=%(refname:short)"])
        .await
        .map(|out| out.lines().map(str::to_owned).collect())
        .unwrap_or_default();
    let worktrees = worktrees(path).await;
    RepoView {
        id,
        name: folder_name(path),
        path: path.to_string_lossy().into_owned(),
        has_commits: has_commits(&worktrees),
        worktrees,
        branches,
        wsl,
    }
}

fn has_commits(worktrees: &[Worktree]) -> bool {
    worktrees
        .iter()
        .find(|w| w.original)
        .is_some_and(|w| w.head.bytes().any(|b| b != b'0'))
}

pub fn folders(path: &str) -> Result<FolderList, RepoProblem> {
    let dir = Path::new(path);
    if !dir.is_absolute() {
        return Err(RepoProblem::NotAbsolute(path.to_owned()));
    }
    if !dir.is_dir() {
        return Err(RepoProblem::NotFound(path.to_owned()));
    }
    let mut folders: Vec<Folder> = std::fs::read_dir(dir)
        .map_err(|e| RepoProblem::Git(e.to_string()))?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .map(|entry| {
            (
                entry.file_name().to_string_lossy().into_owned(),
                entry.path(),
            )
        })
        .filter(|(name, _)| !name.starts_with('.'))
        .map(|(name, path)| Folder {
            git: path.join(".git").exists(),
            name,
        })
        .collect();
    folders.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(FolderList {
        path: path.to_owned(),
        parent: dir.parent().map(|p| p.to_string_lossy().into_owned()),
        folders,
        distro: wsl::of(dir).map(|w| w.distro),
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
    if !dir.is_absolute() {
        return Err(RepoProblem::NotAbsolute(parent.to_owned()));
    }
    Ok(dir.join(name))
}

fn repo_name(url: &str) -> &str {
    let url = url.trim_end_matches('/');
    let leaf = url.rsplit(['/', ':']).next().unwrap_or_default();
    leaf.strip_suffix(".git").unwrap_or(leaf)
}

pub async fn worktrees(repo: &Path) -> Vec<Worktree> {
    listed(repo).await.unwrap_or_else(|_| {
        vec![Worktree {
            path: repo.to_string_lossy().into_owned(),
            head: String::new(),
            branch: None,
            original: true,
        }]
    })
}

pub async fn listed(repo: &Path) -> Result<Vec<Worktree>, String> {
    let porcelain = git::run(repo, &["worktree", "list", "--porcelain"]).await?;
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

pub async fn find(repo: &Path, path: &str) -> Result<Worktree, RepoProblem> {
    listed(repo)
        .await
        .map_err(RepoProblem::Git)?
        .into_iter()
        .find(|w| w.path == path)
        .ok_or_else(|| RepoProblem::NotInRepo(path.to_owned()))
}

pub async fn top_of(cwd: &Path) -> Result<String, String> {
    let top = git::run(cwd, &["rev-parse", "--show-toplevel"]).await?;
    Ok(git::from_git_path(cwd, top.trim()))
}

pub async fn create(
    repo: &Path,
    from: &Path,
    workspace: &Path,
    branch: &str,
    base: &str,
) -> Result<String, RepoProblem> {
    let branch = branch.trim();
    let leaf = branch.rsplit('/').next().unwrap_or_default();
    if leaf.is_empty() {
        return Err(RepoProblem::EmptyBranch);
    }
    if !has_commits(&worktrees(repo).await) {
        return Err(RepoProblem::NoCommit);
    }
    if !workspace.is_absolute() {
        return Err(RepoProblem::NotAbsolute(
            workspace.to_string_lossy().into_owned(),
        ));
    }
    let distro = |dir: &Path| wsl::of(dir).map(|w| w.distro.to_lowercase());
    if distro(repo) != distro(workspace) {
        return Err(RepoProblem::OtherPlace(
            workspace.to_string_lossy().into_owned(),
        ));
    }
    if !workspace.is_dir() {
        return Err(RepoProblem::NoWorkspace(
            workspace.to_string_lossy().into_owned(),
        ));
    }
    let parent = workspace.join(folder_name(repo));
    std::fs::create_dir_all(&parent).map_err(|e| RepoProblem::Git(e.to_string()))?;
    let path = parent.join(leaf).to_string_lossy().into_owned();
    let target = git::to_git_path(from, &path);
    git::run(from, &["worktree", "add", "-b", branch, &target, base])
        .await
        .map_err(RepoProblem::Git)?;
    Ok(listed_path(repo, &path, &target, worktrees(repo).await))
}

fn listed_path(repo: &Path, path: &str, target: &str, listed: Vec<Worktree>) -> String {
    let made = Path::new(path).canonicalize().ok();
    listed
        .into_iter()
        .map(|w| w.path)
        .find(|p| made.is_some() && Path::new(p).canonicalize().ok() == made)
        .unwrap_or_else(|| git::from_git_path(repo, target))
}

pub async fn changes(worktree: &Worktree) -> Result<Vec<String>, RepoProblem> {
    let path = Path::new(&worktree.path);
    if !path.is_dir() {
        return Ok(Vec::new());
    }
    let status = git::run(path, &["status", "--porcelain"])
        .await
        .map_err(RepoProblem::Git)?;
    Ok(status.lines().map(str::to_owned).collect())
}

pub async fn remove(repo: &Path, worktree: &Worktree) -> Result<(), RepoProblem> {
    git_remove(repo, worktree, &["--force"]).await
}

pub async fn remove_clean(repo: &Path, worktree: &Worktree) -> Result<(), RepoProblem> {
    git_remove(repo, worktree, &[]).await
}

async fn git_remove(repo: &Path, worktree: &Worktree, extra: &[&str]) -> Result<(), RepoProblem> {
    if worktree.original {
        return Err(RepoProblem::Original);
    }
    let target = git::to_git_path(repo, &worktree.path);
    let mut args = vec!["worktree", "remove"];
    args.extend_from_slice(extra);
    args.push(&target);
    git::run(repo, &args).await.map_err(RepoProblem::Git)?;
    Ok(())
}

pub async fn rename(repo: &Path, worktree: &Worktree, name: &str) -> Result<(), RepoProblem> {
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
    git::run(repo, &["branch", "-m", "--", branch, name])
        .await
        .map_err(RepoProblem::Git)?;
    Ok(())
}

pub async fn delete_branch(repo: &Path, branch: &str) -> Result<(), RepoProblem> {
    git::run(repo, &["branch", "-D", "--", branch])
        .await
        .map_err(RepoProblem::Git)?;
    Ok(())
}

pub async fn discard(repo: &Path, path: &str) -> Result<(), RepoProblem> {
    let made = find(repo, path).await?;
    remove(repo, &made).await?;
    match &made.branch {
        Some(branch) => delete_branch(repo, branch).await,
        None => Ok(()),
    }
}

async fn original_checkout(folder: &Path) -> Option<PathBuf> {
    git::run(folder, &["rev-parse", "--show-toplevel"])
        .await
        .ok()?;
    let first = worktrees(folder).await.into_iter().next()?;
    Some(PathBuf::from(first.path))
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn setup_script(worktree: &Path) -> Option<String> {
    let text = std::fs::read_to_string(worktree.join("orca.yaml")).ok()?;
    let docs = yaml_rust2::YamlLoader::load_from_str(&text).ok()?;
    let script = docs.first()?["scripts"]["setup"].as_str()?;
    (!script.trim().is_empty()).then(|| script.to_owned())
}

pub fn default_workspace() -> std::io::Result<PathBuf> {
    workspace_in(&loom_home()?)
}

pub fn workspace_in(loom: &Path) -> std::io::Result<PathBuf> {
    let dir = loom.join("workspaces");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn loom_home() -> std::io::Result<PathBuf> {
    let home =
        std::env::home_dir().ok_or_else(|| std::io::Error::other("홈 폴더를 찾지 못했다"))?;
    let dir = home.join("loom");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::git::test_run as run;

    fn repo(dir: &Path) -> PathBuf {
        let repo = dir.join("app");
        std::fs::create_dir(&repo).unwrap();
        run(&repo, &["init", "-q", "-b", "main"]);
        run(&repo, &["commit", "-q", "--allow-empty", "-m", "a"]);
        PathBuf::from(run(&repo, &["rev-parse", "--show-toplevel"]))
    }

    fn repos(dir: &Path) -> Repos {
        Repos::new(crate::db::open(&dir.join("t.db")).unwrap())
    }

    #[test]
    fn porcelain_marks_first_as_original_and_detached_head() {
        let porcelain = "worktree /nowhere/repo\nHEAD aaa\nbranch refs/heads/main\n\n\
                         worktree /nowhere/wt\nHEAD bbb\ndetached\n";
        assert_eq!(
            parse_worktrees(porcelain),
            [
                Worktree {
                    path: "/nowhere/repo".into(),
                    head: "aaa".into(),
                    branch: Some("main".into()),
                    original: true
                },
                Worktree {
                    path: "/nowhere/wt".into(),
                    head: "bbb".into(),
                    branch: None,
                    original: false
                },
            ]
        );
    }

    #[tokio::test]
    async fn add_keeps_one_row_per_repository_and_refuses_other_folders() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(dir.path());
        let wt = dir.path().join("wt");
        run(
            &repo,
            &["worktree", "add", "-q", "-b", "feat", wt.to_str().unwrap()],
        );
        let repos = repos(dir.path());
        let id = repos.add(repo.to_str().unwrap()).await.unwrap();
        assert_eq!(repos.add(wt.to_str().unwrap()).await, Ok(id));
        assert_eq!(repos.list().unwrap(), [(id, repo.clone())]);

        let plain = dir.path().join("plain");
        std::fs::create_dir(&plain).unwrap();
        let plain = plain.to_str().unwrap();
        assert_eq!(
            repos.add(plain).await,
            Err(RepoProblem::NotGit(plain.into()))
        );
        assert_eq!(
            repos.add("/nowhere/at/all").await,
            Err(RepoProblem::NotFound("/nowhere/at/all".into()))
        );
        assert_eq!(repos.path(RepoId(99)), Err(RepoProblem::NoRepo));
    }

    #[tokio::test]
    async fn remove_refuses_while_nodes_remain_and_keeps_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(dir.path());
        let repos = repos(dir.path());
        let id = repos.add(repo.to_str().unwrap()).await.unwrap();
        let graph = repos.db.clone();
        graph
            .lock()
            .execute(
                r#"INSERT INTO nodes (name, agent, state, repo) VALUES ('shell-1', 'null', '{"kind":"running"}', ?1)"#,
                [id.0],
            )
            .unwrap();
        assert_eq!(repos.remove(id), Err(RepoProblem::HasNodes(1)));
        assert_eq!(repos.list().unwrap(), [(id, repo.clone())]);

        graph.lock().execute("DELETE FROM nodes", []).unwrap();
        assert_eq!(repos.remove(id), Ok(()));
        assert_eq!(repos.list().unwrap(), []);
        assert!(repo.join(".git").is_dir());
        assert_eq!(repos.remove(id), Err(RepoProblem::NoRepo));
    }

    #[test]
    fn setup_script_is_scripts_setup_of_orca_yaml() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(setup_script(dir.path()), None);
        let yaml = dir.path().join("orca.yaml");
        std::fs::write(&yaml, "scripts:\n  setup: |\n    a\n    b\n  archive: c\n").unwrap();
        assert_eq!(setup_script(dir.path()), Some("a\nb\n".into()));
        std::fs::write(&yaml, "scripts:\n  archive: c\n").unwrap();
        assert_eq!(setup_script(dir.path()), None);
        std::fs::write(&yaml, "scripts:\n  setup: '  '\n").unwrap();
        assert_eq!(setup_script(dir.path()), None);
    }

    #[tokio::test]
    async fn create_then_remove_a_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(dir.path());
        let workspace = dir.path().join("ws");
        std::fs::create_dir(&workspace).unwrap();
        let path = create(&repo, &repo, &workspace, "me/feat-1", "main")
            .await
            .unwrap();
        assert!(path.ends_with("/ws/app/feat-1"), "{path}");
        let view = view(RepoId(1), &repo, None).await;
        assert_eq!(view.name, "app");
        assert_eq!(view.branches, ["main", "me/feat-1"]);
        assert_eq!(
            view.worktrees
                .iter()
                .map(|w| (w.branch.as_deref(), w.original))
                .collect::<Vec<_>>(),
            [(Some("main"), true), (Some("me/feat-1"), false)]
        );

        std::fs::write(Path::new(&path).join("new.txt"), "x").unwrap();
        let made = find(&repo, &path).await.unwrap();
        assert_eq!(made.branch.as_deref(), Some("me/feat-1"));
        assert_eq!(changes(&made).await.unwrap(), ["?? new.txt"]);
        assert!(matches!(
            remove_clean(&repo, &made).await,
            Err(RepoProblem::Git(_))
        ));
        assert!(Path::new(&path).join("new.txt").exists());
        assert_eq!(remove(&repo, &made).await, Ok(()));
        assert!(!Path::new(&path).exists());
        assert_eq!(worktrees(&repo).await.len(), 1);
        assert_eq!(run(&repo, &["branch", "--list", "me/feat-1"]), "me/feat-1");
        delete_branch(&repo, "me/feat-1").await.unwrap();
        assert_eq!(run(&repo, &["branch", "--list", "me/feat-1"]), "");
    }

    #[test]
    fn a_made_worktree_gets_the_listed_text_when_the_list_does_not_show_it() {
        let repo = r"\\wsl.localhost\NoSuchDistro\home\me\src\loom";
        let path = r"\\wsl.localhost\NoSuchDistro\home\me\loom\workspaces\loom\한글경로";
        let listed = if cfg!(windows) {
            "//wsl.localhost/NoSuchDistro/home/me/loom/workspaces/loom/한글경로"
        } else {
            path
        };
        let original = Worktree {
            path: repo.into(),
            head: String::new(),
            branch: None,
            original: true,
        };
        let target = git::to_git_path(Path::new(repo), path);
        assert_eq!(
            listed_path(Path::new(repo), path, &target, vec![original]),
            listed
        );
    }

    #[tokio::test]
    async fn discard_removes_only_the_made_worktree_and_its_branch() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(dir.path());
        let workspace = dir.path().join("ws");
        std::fs::create_dir(&workspace).unwrap();
        let kept = create(&repo, &repo, &workspace, "kept", "main")
            .await
            .unwrap();
        let made = create(&repo, &repo, &workspace, "me/made", "main")
            .await
            .unwrap();
        assert_eq!(discard(&repo, &made).await, Ok(()));
        assert!(!Path::new(&made).exists());
        assert_eq!(run(&repo, &["branch", "--list", "me/made"]), "");
        assert_eq!(
            view(RepoId(1), &repo, None).await.branches,
            ["kept", "main"]
        );
        assert!(find(&repo, &kept).await.is_ok());
        assert_eq!(
            discard(&repo, &made).await,
            Err(RepoProblem::NotInRepo(made.clone()))
        );
        assert_eq!(
            discard(&repo, repo.to_str().unwrap()).await,
            Err(RepoProblem::Original)
        );
    }

    #[tokio::test]
    async fn remove_refuses_original() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(dir.path());
        let original = find(&repo, repo.to_str().unwrap()).await.unwrap();
        assert_eq!(remove(&repo, &original).await, Err(RepoProblem::Original));
        assert!(repo.exists());
        assert_eq!(
            find(&repo, "/nowhere/wt").await,
            Err(RepoProblem::NotInRepo("/nowhere/wt".into()))
        );
    }

    #[tokio::test]
    async fn find_reports_the_git_failure_and_the_screen_list_keeps_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(dir.path());
        std::fs::rename(repo.join(".git"), dir.path().join("moved.git")).unwrap();
        let path = repo.to_str().unwrap();
        let Err(RepoProblem::Git(e)) = find(&repo, path).await else {
            panic!("find hid the git failure");
        };
        assert!(e.contains("not a git repository"), "{e}");
        assert_eq!(worktrees(&repo).await.len(), 1);
    }

    #[tokio::test]
    async fn rename_keeps_the_folder_and_refuses_original_detached_bad_and_taken_names() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(dir.path());
        let workspace = dir.path().join("ws");
        std::fs::create_dir(&workspace).unwrap();
        let path = create(&repo, &repo, &workspace, "feat", "main")
            .await
            .unwrap();
        let made = find(&repo, &path).await.unwrap();
        assert_eq!(rename(&repo, &made, " me/new ").await, Ok(()));
        let renamed = find(&repo, &path).await.unwrap();
        assert_eq!(renamed.branch.as_deref(), Some("me/new"));
        assert_eq!(
            view(RepoId(1), &repo, None).await.branches,
            ["main", "me/new"]
        );

        assert_eq!(
            rename(&repo, &renamed, " ").await,
            Err(RepoProblem::EmptyBranch)
        );
        for bad in ["a..b", "-x", "main"] {
            let Err(RepoProblem::Git(e)) = rename(&repo, &renamed, bad).await else {
                panic!("{bad}")
            };
            assert!(
                e.contains("not a valid branch name") || e.contains("already exists"),
                "{e}"
            );
        }
        let original = find(&repo, repo.to_str().unwrap()).await.unwrap();
        assert_eq!(
            rename(&repo, &original, "x").await,
            Err(RepoProblem::Original)
        );
        run(Path::new(&path), &["checkout", "--detach"]);
        let detached = find(&repo, &path).await.unwrap();
        assert_eq!(
            rename(&repo, &detached, "x").await,
            Err(RepoProblem::Detached)
        );
    }

    #[tokio::test]
    async fn remove_a_worktree_whose_folder_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(dir.path());
        let workspace = dir.path().join("ws");
        std::fs::create_dir(&workspace).unwrap();
        let path = create(&repo, &repo, &workspace, "feat", "main")
            .await
            .unwrap();
        std::fs::remove_dir_all(&path).unwrap();
        let gone = find(&repo, &path).await.unwrap();
        assert_eq!(changes(&gone).await, Ok(Vec::new()));
        assert_eq!(remove(&repo, &gone).await, Ok(()));
        assert_eq!(worktrees(&repo).await.len(), 1);
        assert_eq!(run(&repo, &["branch", "--list", "feat"]), "feat");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn create_refuses_a_workspace_in_another_place() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(dir.path());
        let ws = Path::new(r"\\wsl.localhost\NoSuchDistro\home\me\loom\workspaces");
        assert_eq!(
            create(&repo, &repo, ws, "feat", "main").await,
            Err(RepoProblem::OtherPlace(ws.to_string_lossy().into_owned()))
        );
    }

    #[tokio::test]
    async fn create_refuses_empty_branch_relative_workspace_and_taken_branch() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(dir.path());
        let workspace = dir.path().join("ws");
        std::fs::create_dir(&workspace).unwrap();
        assert_eq!(
            create(&repo, &repo, &workspace, " ", "main").await,
            Err(RepoProblem::EmptyBranch)
        );
        assert_eq!(
            create(&repo, &repo, Path::new("ws"), "feat", "main").await,
            Err(RepoProblem::NotAbsolute("ws".into()))
        );
        let missing = dir.path().join("missing");
        assert_eq!(
            create(&repo, &repo, &missing, "feat", "main").await,
            Err(RepoProblem::NoWorkspace(
                missing.to_string_lossy().into_owned()
            ))
        );
        let Err(RepoProblem::Git(e)) = create(&repo, &repo, &workspace, "main", "main").await
        else {
            panic!()
        };
        assert!(e.contains("already exists"), "{e}");
    }

    #[test]
    fn folders_lists_child_folders_by_name_and_marks_git() {
        let dir = tempfile::tempdir().unwrap();
        repo(dir.path());
        std::fs::create_dir(dir.path().join("b-plain")).unwrap();
        std::fs::create_dir(dir.path().join(".hidden")).unwrap();
        std::fs::write(dir.path().join("file.txt"), "x").unwrap();
        let path = dir.path().to_str().unwrap();
        let list = folders(path).unwrap();
        assert_eq!(list.path, path);
        assert_eq!(
            list.parent.as_deref(),
            dir.path().parent().and_then(Path::to_str)
        );
        assert_eq!(
            list.folders
                .iter()
                .map(|f| (f.name.as_str(), f.git))
                .collect::<Vec<_>>(),
            [("app", true), ("b-plain", false)]
        );
        assert_eq!(
            folders("rel").err(),
            Some(RepoProblem::NotAbsolute("rel".into()))
        );
        let nowhere = if cfg!(windows) {
            r"C:\nowhere\at\all"
        } else {
            "/nowhere/at/all"
        };
        assert_eq!(
            folders(nowhere).err(),
            Some(RepoProblem::NotFound(nowhere.into()))
        );
    }

    #[test]
    fn repo_name_is_last_segment_without_git_suffix() {
        assert_eq!(repo_name("https://github.com/team/app.git"), "app");
        assert_eq!(repo_name("https://github.com/team/app/"), "app");
        assert_eq!(repo_name("git@github.com:team/app.git"), "app");
        assert_eq!(repo_name("host:app"), "app");
        assert_eq!(repo_name("/srv/git/app.git"), "app");
    }

    #[tokio::test]
    async fn clone_adds_the_copy_and_keeps_existing_folders() {
        let dir = tempfile::tempdir().unwrap();
        let source = repo(dir.path());
        let parent = dir.path().join("p").join("q");
        let parent_text = parent.to_str().unwrap();
        let repos = repos(dir.path());
        let id = repos
            .clone(source.to_str().unwrap(), parent_text)
            .await
            .unwrap();
        let copy = parent.join("app");
        assert_eq!(
            repos.path(id).unwrap().canonicalize().unwrap(),
            copy.canonicalize().unwrap()
        );
        assert!(view(id, &copy, None).await.has_commits);
        assert_eq!(git::calls_during(view(id, &copy, None)).await.0, 2);

        assert_eq!(
            repos.clone(source.to_str().unwrap(), parent_text).await,
            Err(RepoProblem::NotEmpty(copy.to_string_lossy().into_owned()))
        );
        assert!(copy.join(".git").exists());

        let Err(RepoProblem::Git(_)) = repos.clone("/nowhere/gone.git", parent_text).await else {
            panic!()
        };
        assert!(!parent.join("gone").exists());
        std::fs::create_dir(parent.join("empty")).unwrap();
        let Err(RepoProblem::Git(_)) = repos.clone("/nowhere/empty.git", parent_text).await else {
            panic!()
        };
        assert!(parent.join("empty").is_dir());

        assert_eq!(
            repos.clone("https://h/x/", "rel").await,
            Err(RepoProblem::NotAbsolute("rel".into()))
        );
    }

    #[tokio::test]
    async fn create_inits_without_a_commit_and_refuses_existing_folders() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().to_str().unwrap();
        let repos = repos(dir.path());
        let id = repos.create(parent, " my-app ").await.unwrap();
        let made = repos.path(id).unwrap();
        assert!(made.ends_with("my-app"), "{made:?}");
        let made_view = view(id, &made, None).await;
        assert!(!made_view.has_commits);
        assert_eq!(made_view.branches, Vec::<String>::new());

        let workspace = dir.path().join("ws");
        std::fs::create_dir(&workspace).unwrap();
        assert_eq!(
            create(&made, &made, &workspace, "feat", "HEAD").await,
            Err(RepoProblem::NoCommit)
        );
        assert!(!workspace.join("my-app").exists());
        run(&made, &["commit", "-q", "--allow-empty", "-m", "a"]);
        assert!(view(id, &made, None).await.has_commits);
        run(&made, &["checkout", "-q", "--detach"]);
        assert!(view(id, &made, None).await.has_commits);
        create(&made, &made, &workspace, "feat", "HEAD")
            .await
            .unwrap();

        let taken = dir.path().join("taken");
        std::fs::create_dir(&taken).unwrap();
        std::fs::write(taken.join("keep.txt"), "x").unwrap();
        assert_eq!(
            repos.create(parent, "taken").await,
            Err(RepoProblem::Exists(taken.to_string_lossy().into_owned()))
        );
        assert!(!taken.join(".git").exists());
        assert!(taken.join("keep.txt").exists());

        for bad in ["", "..", "a/b", "a\\b"] {
            assert_eq!(
                repos.create(parent, bad).await,
                Err(RepoProblem::BadName(bad.into()))
            );
        }

        let missing = dir.path().join("new").join("deep");
        let id = repos
            .create(missing.to_str().unwrap(), "fresh")
            .await
            .unwrap();
        assert_eq!(repos.path(id).unwrap(), missing.join("fresh"));
        assert!(missing.join("fresh").join(".git").exists());
    }

    #[tokio::test]
    async fn head_base_starts_from_the_calling_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(dir.path());
        let workspace = dir.path().join("ws");
        std::fs::create_dir(&workspace).unwrap();
        let parent = create(&repo, &repo, &workspace, "parent", "main")
            .await
            .unwrap();
        run(
            Path::new(&parent),
            &["commit", "-q", "--allow-empty", "-m", "b"],
        );
        let child = create(&repo, Path::new(&parent), &workspace, "child", "HEAD")
            .await
            .unwrap();
        assert_eq!(run(Path::new(&child), &["log", "-1", "--format=%s"]), "b");
    }

    #[tokio::test]
    async fn an_empty_sha256_repo_has_no_commits() {
        let dir = tempfile::tempdir().unwrap();
        crate::git::test_run(
            dir.path(),
            &["init", "-q", "--object-format=sha256", "-b", "main"],
        );
        assert!(!view(RepoId(1), dir.path(), None).await.has_commits);
    }
}
