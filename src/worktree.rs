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
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct Folder {
    pub name: String,
    /// 안에 `.git`이 있다.
    pub git: bool,
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
    /// 없는 저장소 id다.
    NoRepo,
    /// 이 저장소의 워크트리가 아니다.
    NotInRepo(String),
    /// 원래 체크아웃은 지우지 않는다. git도 지우지 못한다.
    Original,
    EmptyBranch,
    /// 첫 커밋 전 저장소에서는 워크트리를 만들지 못한다.
    NoCommit,
    /// 새 저장소 폴더 이름으로 쓸 수 없다. 복제면 주소에서 얻은 이름이다.
    BadName(String),
    /// 복제할 폴더가 이미 있고 비어 있지 않다. 그 폴더는 건드리지 않는다.
    NotEmpty(String),
    /// 새 프로젝트 폴더가 이미 있다. 그 폴더는 건드리지 않는다.
    Exists(String),
    /// git이 실패했다. 값은 git의 오류 글이다.
    Git(String),
}

impl fmt::Display for RepoProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RepoProblem::NotFound(path) => write!(f, "{path}: 데몬이 보는 파일 시스템에 없다."),
            RepoProblem::NotGit(path) => write!(f, "{path}: git 저장소가 아니다."),
            RepoProblem::NotAbsolute(path) => write!(f, "{path}: 절대 경로를 적는다."),
            RepoProblem::NoWorkspace(path) => write!(f, "{path}: 워크스페이스 폴더가 없다."),
            RepoProblem::NoRepo => write!(f, "없는 저장소다."),
            RepoProblem::NotInRepo(path) => write!(f, "{path}: 이 저장소의 워크트리가 아니다."),
            RepoProblem::Original => write!(f, "원래 체크아웃은 지우지 않는다."),
            RepoProblem::EmptyBranch => write!(f, "브랜치 이름을 적는다."),
            RepoProblem::NoCommit => write!(f, "첫 커밋 전 저장소다. 커밋한 뒤 워크트리를 만든다."),
            RepoProblem::BadName(name) => write!(f, "'{name}': 폴더 이름으로 쓸 수 없다."),
            RepoProblem::NotEmpty(path) => write!(f, "{path}: 이미 있고 비어 있지 않다."),
            RepoProblem::Exists(path) => write!(f, "{path}: 이미 있다."),
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

    /// `url`을 `<parent>/<주소 마지막 마디에서 .git을 뺀 이름>`에 복제하고 더한다.
    /// 그 폴더가 있고 비어 있으면 그 안에 복제한다. 실패하면 git이 자기가 만든 것만 지운다. loom은 지우지 않는다.
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

    /// `<parent>/<name>` 폴더를 새로 만들고 `git init`한 뒤 더한다. 첫 커밋은 만들지 않는다.
    pub async fn create(&self, parent: &str, name: &str) -> Result<RepoId, RepoProblem> {
        let target = new_folder(parent, name.trim())?;
        let text = target.to_string_lossy().into_owned();
        if target.exists() {
            return Err(RepoProblem::Exists(text));
        }
        std::fs::create_dir(&target).map_err(|e| RepoProblem::Git(e.to_string()))?;
        if let Err(e) = git::run(&target, &["init", "-q"]).await {
            // 방금 만든 폴더다. 비어 있을 때만 지워진다.
            let _ = std::fs::remove_dir(&target);
            return Err(RepoProblem::Git(e));
        }
        self.add(&text).await
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
        has_commits: has_commits(path).await,
    }
}

async fn has_commits(repo: &Path) -> bool {
    git::run(repo, &["rev-parse", "--verify", "-q", "HEAD"])
        .await
        .is_ok()
}

/// `path`의 자식 폴더 목록. 심볼릭 링크는 따라간다.
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
    })
}

/// 복제·새 프로젝트가 만들 폴더 `<parent>/<name>`. `parent`는 있는 절대 경로이고 `name`은 한 마디다.
fn new_folder(parent: &str, name: &str) -> Result<PathBuf, RepoProblem> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err(RepoProblem::BadName(name.to_owned()));
    }
    let dir = Path::new(parent);
    if !dir.is_absolute() {
        return Err(RepoProblem::NotAbsolute(parent.to_owned()));
    }
    if !dir.is_dir() {
        return Err(RepoProblem::NotFound(parent.to_owned()));
    }
    Ok(dir.join(name))
}

/// git이 `git clone <url>`에서 고르는 폴더 이름과 같다: 마지막 마디(`/`·`:` 뒤)에서 `.git`을 뺀다.
fn repo_name(url: &str) -> &str {
    let url = url.trim_end_matches('/');
    let leaf = url.rsplit(['/', ':']).next().unwrap_or_default();
    leaf.strip_suffix(".git").unwrap_or(leaf)
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
    if !has_commits(repo).await {
        return Err(RepoProblem::NoCommit);
    }
    if !workspace.is_absolute() {
        return Err(RepoProblem::NotAbsolute(
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

/// 지우면 잃는 변경: 커밋하지 않은 파일과 추적하지 않는 파일. 줄마다 `git status --porcelain` 한 줄이다.
/// 폴더가 이미 없으면 잃을 것이 없다. 그 폴더에서는 git이 돌지 못한다.
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

/// 워크트리를 지운다: 커밋하지 않은 변경이 있어도 폴더째 지운다(`--force`, Orca와 같다). 브랜치는 남긴다.
/// 폴더가 이미 없어도 git이 등록을 지운다. 원래 체크아웃은 거절한다.
pub async fn remove(repo: &Path, worktree: &Worktree) -> Result<(), RepoProblem> {
    if worktree.original {
        return Err(RepoProblem::Original);
    }
    git::run(repo, &["worktree", "remove", "--force", &worktree.path])
        .await
        .map_err(RepoProblem::Git)?;
    Ok(())
}

/// 병합하지 않은 커밋이 있어도 지운다(`-D`). 사람이 삭제 창에서 고를 때만 부른다.
pub async fn delete_branch(repo: &Path, branch: &str) -> Result<(), RepoProblem> {
    git::run(repo, &["branch", "-D", "--", branch])
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

/// 새 워크트리에서 돌릴 설정 스크립트: 그 워크트리의 `orca.yaml`에 있는 `scripts.setup`.
/// Orca와 같은 파일과 같은 키를 읽어 저장소 설정 하나가 둘 다에 쓰인다. 파일이 없거나 읽지 못하면 없다(Orca와 같다).
pub fn setup_script(worktree: &Path) -> Option<String> {
    let text = std::fs::read_to_string(worktree.join("orca.yaml")).ok()?;
    let docs = yaml_rust2::YamlLoader::load_from_str(&text).ok()?;
    let script = docs.first()?["scripts"]["setup"].as_str()?;
    (!script.trim().is_empty()).then(|| script.to_owned())
}

/// Orca 기본값(`<홈>/orca/workspaces`)과 같은 모양이다. Orca처럼 없으면 만든다. 사람이 고른 폴더는 만들지 않는다.
/// 근거: docs/design.md 「저장소와 워크트리」.
pub fn default_workspace() -> std::io::Result<PathBuf> {
    let dir = loom_home()?.join("workspaces");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// `<홈>/loom`. [저장소 추가] 폴더 목록이 처음 여는 폴더다. 없으면 만든다.
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

        // 커밋하지 않은 변경과 추적하지 않는 파일이 있어도 지운다. 브랜치는 남긴다.
        std::fs::write(Path::new(&path).join("new.txt"), "x").unwrap();
        let made = find(&repo, &path).await.unwrap();
        assert_eq!(made.branch.as_deref(), Some("me/feat-1"));
        assert_eq!(changes(&made).await.unwrap(), ["?? new.txt"]);
        assert_eq!(remove(&repo, &made).await, Ok(()));
        assert!(!Path::new(&path).exists());
        assert_eq!(worktrees(&repo).await.len(), 1);
        assert_eq!(run(&repo, &["branch", "--list", "me/feat-1"]), "me/feat-1");
        delete_branch(&repo, "me/feat-1").await.unwrap();
        assert_eq!(run(&repo, &["branch", "--list", "me/feat-1"]), "");
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

    /// 폴더를 loom 밖에서 지워도(`rm -rf`) git은 워크트리를 적어 둔다. 잃을 것이 없고, 지우면 등록이 사라진다.
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
        assert_eq!(
            folders("/nowhere/at/all").err(),
            Some(RepoProblem::NotFound("/nowhere/at/all".into()))
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
        let parent = dir.path().join("p");
        std::fs::create_dir(&parent).unwrap();
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
        assert!(view(id, &copy).await.has_commits);

        // 대상이 비어 있지 않으면 git을 부르지 않고 거절한다. 원래 파일은 남는다.
        assert_eq!(
            repos.clone(source.to_str().unwrap(), parent_text).await,
            Err(RepoProblem::NotEmpty(copy.to_string_lossy().into_owned()))
        );
        assert!(copy.join(".git").exists());

        // 원본이 없으면 실패한다. 새 폴더는 남지 않고, 미리 있던 빈 폴더는 남는다.
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
        let made_view = view(id, &made).await;
        assert!(!made_view.has_commits);
        assert_eq!(made_view.branches, Vec::<String>::new());

        // 첫 커밋 전에는 워크트리를 만들지 않는다. 커밋 뒤에는 만든다.
        let workspace = dir.path().join("ws");
        std::fs::create_dir(&workspace).unwrap();
        assert_eq!(
            create(&made, &made, &workspace, "feat", "HEAD").await,
            Err(RepoProblem::NoCommit)
        );
        assert!(!workspace.join("my-app").exists());
        run(&made, &["commit", "-q", "--allow-empty", "-m", "a"]);
        assert!(view(id, &made).await.has_commits);
        // 분리된 HEAD는 브랜치 목록과 관계없이 커밋이 있다.
        run(&made, &["checkout", "-q", "--detach"]);
        assert!(view(id, &made).await.has_commits);
        create(&made, &made, &workspace, "feat", "HEAD")
            .await
            .unwrap();

        // 이미 있는 폴더는 건드리지 않는다.
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
    }

    /// `loom spawn --worktree`: 기준이 `HEAD`면 부모 워크트리가 체크아웃한 커밋에서 갈라진다.
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
}
