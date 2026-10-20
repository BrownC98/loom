//! 저장소와 워크트리: 사람이 더한 저장소 목록(SQLite)과, 저장소마다 `git worktree`로 만들고 닫는 워크트리.
//! 워크트리 묶음의 원천은 `git worktree list`다. loom은 따로 저장하지 않는다.
//! 근거: docs/design.md 「저장소와 워크트리」.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::git;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RepoId(pub u32);

#[derive(Serialize, TS)]
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
}

#[derive(Serialize, TS, PartialEq, Debug)]
#[ts(export)]
pub struct Worktree {
    /// 절대 경로. 노드는 이 경로로 묶음을 가리킨다.
    pub path: String,
    /// 체크아웃한 커밋. git을 읽지 못하면 비어 있다.
    pub head: String,
    /// 분리된 HEAD면 없다.
    pub branch: Option<String>,
    /// 원래 체크아웃이면 참이다. 닫을 수 없다.
    pub original: bool,
}

/// 저장소·워크트리 요청을 거절한 이유. 화면에는 코드와 값(JSON)이, 에이전트에게는 한국어 문장(`Display`)이 간다.
#[derive(Debug, PartialEq, Serialize, TS)]
#[serde(tag = "code", content = "value", rename_all = "camelCase")]
#[ts(export)]
pub enum RepoProblem {
    /// 데몬이 보는 파일 시스템에 이 폴더가 없다. 개발 컨테이너의 데몬은 `compose.yaml`이 넣은 폴더만 본다.
    NotFound(String),
    NotGit(String),
    /// 워크스페이스 폴더는 절대 경로다.
    NotAbsolute(String),
    /// 없는 저장소 id다.
    NoRepo,
    /// 이 저장소의 워크트리가 아니다.
    NotInRepo(String),
    /// 원래 체크아웃은 닫지 않는다.
    Original,
    /// 커밋하지 않은 변경이 있다. 값은 바뀐 파일 수다.
    Dirty(u32),
    /// 안에 `running` 노드가 있다. 값은 그 수다.
    Running(u32),
    EmptyBranch,
    /// git이 실패했다. 값은 git의 오류 글이다.
    Git(String),
}

impl fmt::Display for RepoProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RepoProblem::NotFound(path) => write!(f, "{path}: 데몬이 보는 파일 시스템에 없다."),
            RepoProblem::NotGit(path) => write!(f, "{path}: git 저장소가 아니다."),
            RepoProblem::NotAbsolute(path) => write!(f, "{path}: 절대 경로를 적는다."),
            RepoProblem::NoRepo => write!(f, "없는 저장소다."),
            RepoProblem::NotInRepo(path) => write!(f, "{path}: 이 저장소의 워크트리가 아니다."),
            RepoProblem::Original => write!(f, "원래 체크아웃은 닫지 않는다."),
            RepoProblem::Dirty(n) => write!(f, "커밋하지 않은 변경이 {n}개 있다."),
            RepoProblem::Running(n) => write!(f, "running 노드가 {n}개 있다."),
            RepoProblem::EmptyBranch => write!(f, "브랜치 이름을 적는다."),
            RepoProblem::Git(e) => write!(f, "git 오류: {e}"),
        }
    }
}

pub struct Repos {
    db: Mutex<Connection>,
}

impl Repos {
    pub fn new(conn: Connection) -> Self {
        Self {
            db: Mutex::new(conn),
        }
    }

    /// 폴더가 속한 저장소를 더한다. 원래 체크아웃 경로로 저장하므로 같은 저장소의 다른 워크트리를 적어도 하나다.
    pub async fn add(&self, path: &str) -> Result<RepoId, RepoProblem> {
        let folder = Path::new(path);
        if !folder.is_dir() {
            return Err(RepoProblem::NotFound(path.to_owned()));
        }
        let original = original_checkout(folder)
            .await
            .ok_or_else(|| RepoProblem::NotGit(path.to_owned()))?;
        self.insert(&original)
            .map_err(|e| RepoProblem::Git(e.to_string()))
    }

    /// 데몬의 시작 폴더를 더한다. 화면으로 더할 때와 달리 git 저장소가 아니어도 그 폴더 그대로 더한다.
    /// 근거: 데몬은 늘 시작 폴더를 첫 저장소로 보였다(docs/design.md 「저장소와 워크트리」).
    pub async fn add_start(&self, folder: &Path) -> rusqlite::Result<(RepoId, PathBuf)> {
        let original = original_checkout(folder)
            .await
            .unwrap_or_else(|| folder.to_owned());
        Ok((self.insert(&original)?, original))
    }

    fn insert(&self, path: &Path) -> rusqlite::Result<RepoId> {
        let text = path.to_string_lossy();
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO repos (path) VALUES (?1) ON CONFLICT (path) DO NOTHING",
            [&text],
        )?;
        db.query_row("SELECT id FROM repos WHERE path = ?1", [&text], |row| {
            Ok(RepoId(row.get(0)?))
        })
    }

    /// 더한 순서다.
    pub fn list(&self) -> rusqlite::Result<Vec<(RepoId, PathBuf)>> {
        self.db
            .lock()
            .unwrap()
            .prepare("SELECT id, path FROM repos ORDER BY id")?
            .query_map([], |row| {
                Ok((RepoId(row.get(0)?), PathBuf::from(row.get::<_, String>(1)?)))
            })?
            .collect()
    }

    pub fn path(&self, id: RepoId) -> Result<PathBuf, RepoProblem> {
        self.db
            .lock()
            .unwrap()
            .query_row("SELECT path FROM repos WHERE id = ?1", [id.0], |row| {
                row.get::<_, String>(0)
            })
            .optional()
            .map_err(|e| RepoProblem::Git(e.to_string()))?
            .map(PathBuf::from)
            .ok_or(RepoProblem::NoRepo)
    }
}

/// 저장소 하나를 화면에 보일 모양으로 읽는다.
pub async fn view(id: RepoId, path: &Path) -> RepoView {
    let branches = git::run(path, &["branch", "--format=%(refname:short)"])
        .await
        .map(|out| out.lines().map(str::to_owned).collect())
        .unwrap_or_default();
    RepoView {
        id,
        name: folder_name(path),
        path: path.to_string_lossy().into_owned(),
        worktrees: worktrees(path).await,
        branches,
    }
}

/// 첫째가 원래 체크아웃이다(git이 그 순서로 준다). git을 읽지 못하면 `repo` 하나다.
pub async fn worktrees(repo: &Path) -> Vec<Worktree> {
    match git::run(repo, &["worktree", "list", "--porcelain"]).await {
        Ok(porcelain) => parse_worktrees(&porcelain),
        Err(_) => vec![Worktree {
            path: repo.to_string_lossy().into_owned(),
            head: String::new(),
            branch: None,
            original: true,
        }],
    }
}

/// `porcelain`은 `git worktree list --porcelain` 출력이다. 첫 덩어리가 원래 체크아웃이다.
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

/// `path`가 `repo`의 워크트리 가운데 하나면 그 워크트리다.
pub async fn find(repo: &Path, path: &str) -> Result<Worktree, RepoProblem> {
    worktrees(repo)
        .await
        .into_iter()
        .find(|w| w.path == path)
        .ok_or_else(|| RepoProblem::NotInRepo(path.to_owned()))
}

/// 새 브랜치 `branch`를 `base`에서 만들고 `<workspace>/<저장소 이름>/<브랜치 마지막 마디>`에 체크아웃한다.
/// git은 `from` 폴더에서 돈다. 그래서 `base`가 `HEAD`면 `from`이 체크아웃한 커밋이다. 돌려주는 값은 새 워크트리 경로다.
/// 폴더 배치는 Orca의 「워크스페이스 중첩」과 같다.
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
    if !workspace.is_absolute() {
        return Err(RepoProblem::NotAbsolute(
            workspace.to_string_lossy().into_owned(),
        ));
    }
    let parent = workspace.join(folder_name(repo));
    std::fs::create_dir_all(&parent).map_err(|e| RepoProblem::Git(e.to_string()))?;
    let path = parent.join(leaf).to_string_lossy().into_owned();
    git::run(from, &["worktree", "add", "-b", branch, &path, base])
        .await
        .map_err(RepoProblem::Git)?;
    // git이 적는 경로와 같은 글자로 돌려준다. 노드는 이 글자로 묶음을 가리킨다.
    let listed = worktrees(repo).await;
    let made = Path::new(&path);
    Ok(listed
        .into_iter()
        .map(|w| w.path)
        .find(|p| Path::new(p).canonicalize().ok() == made.canonicalize().ok())
        .unwrap_or(path))
}

/// 워크트리를 닫는다: 폴더를 지우고 브랜치는 남긴다. 원래 체크아웃과 커밋하지 않은 변경이 있는 워크트리는 거절한다.
/// `--force`를 쓰지 않으므로 git도 바뀐 파일이 있으면 거절한다. 무시하는 파일(`target/` 등)은 함께 지운다.
pub async fn close(repo: &Path, path: &str) -> Result<(), RepoProblem> {
    if find(repo, path).await?.original {
        return Err(RepoProblem::Original);
    }
    let status = git::run(Path::new(path), &["status", "--porcelain"])
        .await
        .map_err(RepoProblem::Git)?;
    let changed = status.lines().count() as u32;
    if changed > 0 {
        return Err(RepoProblem::Dirty(changed));
    }
    git::run(repo, &["worktree", "remove", path])
        .await
        .map_err(RepoProblem::Git)?;
    Ok(())
}

/// 폴더가 속한 저장소의 원래 체크아웃. git 저장소가 아니면 없다.
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

/// Orca 기본값(`<홈>/orca/workspaces`)과 같은 모양이다. 근거: docs/design.md 「저장소와 워크트리」.
pub fn default_workspace() -> Option<PathBuf> {
    std::env::home_dir().map(|home| home.join("loom").join("workspaces"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    /// 커밋 하나가 있는 `main` 저장소. 경로는 git이 적는 글자(심볼릭 링크를 푼 경로)다.
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
        // 같은 저장소의 다른 워크트리를 적어도 원래 체크아웃 한 줄이다.
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
        // 시작 폴더는 git 저장소가 아니어도 더한다.
        let (start, path) = repos.add_start(Path::new(plain)).await.unwrap();
        assert_eq!(path, Path::new(plain));
        assert_eq!(repos.path(start), Ok(PathBuf::from(plain)));
        assert_eq!(repos.path(RepoId(99)), Err(RepoProblem::NoRepo));
    }

    #[tokio::test]
    async fn create_then_close_a_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(dir.path());
        let workspace = dir.path().join("ws");
        let path = create(&repo, &repo, &workspace, "me/feat-1", "main")
            .await
            .unwrap();
        // `<워크스페이스>/<저장소 이름>/<브랜치 마지막 마디>`.
        assert!(path.ends_with("/ws/app/feat-1"), "{path}");
        let view = view(RepoId(1), &repo).await;
        assert_eq!(view.name, "app");
        assert_eq!(view.branches, ["main", "me/feat-1"]);
        assert_eq!(
            view.worktrees
                .iter()
                .map(|w| (w.branch.as_deref(), w.original))
                .collect::<Vec<_>>(),
            [(Some("main"), true), (Some("me/feat-1"), false)]
        );

        close(&repo, &path).await.unwrap();
        assert!(!Path::new(&path).exists());
        assert_eq!(worktrees(&repo).await.len(), 1);
        // 브랜치는 남긴다.
        assert_eq!(run(&repo, &["branch", "--list", "me/feat-1"]), "me/feat-1");
    }

    #[tokio::test]
    async fn close_refuses_original_dirty_and_unknown_worktrees() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(dir.path());
        let repo_text = repo.to_str().unwrap();
        assert_eq!(close(&repo, repo_text).await, Err(RepoProblem::Original));
        assert_eq!(
            close(&repo, "/nowhere/wt").await,
            Err(RepoProblem::NotInRepo("/nowhere/wt".into()))
        );

        let path = create(&repo, &repo, &dir.path().join("ws"), "feat", "main")
            .await
            .unwrap();
        std::fs::write(Path::new(&path).join("new.txt"), "x").unwrap();
        std::fs::write(Path::new(&path).join("other.txt"), "y").unwrap();
        assert_eq!(close(&repo, &path).await, Err(RepoProblem::Dirty(2)));
        assert!(Path::new(&path).join("new.txt").exists());
    }

    #[tokio::test]
    async fn create_refuses_empty_branch_relative_workspace_and_taken_branch() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(dir.path());
        let workspace = dir.path().join("ws");
        assert_eq!(
            create(&repo, &repo, &workspace, " ", "main").await,
            Err(RepoProblem::EmptyBranch)
        );
        assert_eq!(
            create(&repo, &repo, Path::new("ws"), "feat", "main").await,
            Err(RepoProblem::NotAbsolute("ws".into()))
        );
        let Err(RepoProblem::Git(e)) = create(&repo, &repo, &workspace, "main", "main").await
        else {
            panic!()
        };
        assert!(e.contains("already exists"), "{e}");
    }

    /// `loom spawn --worktree`: 기준이 `HEAD`면 부모 워크트리가 체크아웃한 커밋에서 갈라진다.
    #[tokio::test]
    async fn head_base_starts_from_the_calling_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(dir.path());
        let workspace = dir.path().join("ws");
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
}
