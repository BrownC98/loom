use std::fmt;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ids::RepoId;

#[derive(Serialize, Deserialize, TS, Clone)]
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
    /// 거짓이면 git 없는 폴더다. 노드는 이 폴더에서 돌고, 워크트리와 병합 관문이 없다. 이 필드가 없는 옛 데몬은 git 저장소만 준다.
    #[serde(default = "has_git")]
    pub git: bool,
    /// 저장소가 도는 곳. 경로에서 나온 값이다.
    pub place: PlaceView,
    /// 화면에 보일 경로. WSL은 배포판 안의 경로이고, 홈 아래면 `~/…`다. SSH는 그 PC 안의 경로다.
    pub shown_path: String,
}

fn has_git() -> bool {
    true
}

/// 폴더가 있는 곳: 이 PC, WSL 배포판, SSH 호스트. 근거: 목업 docs/mockups/wsl-place.html ②·②′, docs/mockups/ssh-repo.html ④.
#[derive(Serialize, Deserialize, TS, PartialEq, Debug, Clone)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum PlaceView {
    Pc,
    Wsl { distro: String },
    Ssh { host: String },
}

/// [저장소 추가] 위치 칩: 이 PC의 드라이브와 WSL 배포판. 배포판이 없으면 화면이 드라이브·배포판 칩을 보이지 않는다. [SSH] 칩은 늘 보인다.
#[derive(Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Locations {
    /// `C:\` 꼴. Windows 데몬이 아니면 비었다.
    pub drives: Vec<String>,
    pub distros: Vec<String>,
}

/// [저장소 추가]의 폴더 목록: 데몬이 보는 폴더 하나의 자식 폴더.
#[derive(Serialize, Deserialize, TS)]
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

#[derive(Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Folder {
    pub name: String,
    /// 안에 `.git`이 있다.
    pub git: bool,
}

#[derive(Serialize, Deserialize, TS, PartialEq, Debug, Clone)]
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
#[derive(Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "code", content = "value", rename_all = "camelCase")]
#[ts(export)]
pub enum RepoProblem {
    /// 데몬이 보는 파일 시스템에 이 폴더가 없다.
    NotFound(String),
    /// 이 폴더에 git이 없다. 워크트리, 커밋 그래프, 병합 관문은 git 저장소에서만 쓴다.
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
            RepoProblem::NotGit(path) => write!(
                f,
                "{path} has no git. Worktrees, loom check and loom ready need a git repository. Nodes work in this folder: spawn a child without --worktree."
            ),
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
