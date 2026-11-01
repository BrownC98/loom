use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ids::{NodeId, RepoId};
use crate::repos::Worktree;

/// 자리를 가진 상자. 자리의 기준은 상자마다 다르다.
/// 저장소 상자는 그래프 좌표, 맨 위 묶음은 저장소 상자 안 좌표, 자식 묶음과 카드는 묶음 안 좌표다. 그래서 상자를 옮겨도 안의 자리는 그대로다.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Spot {
    Repo {
        repo: RepoId,
    },
    /// `path`는 `git worktree list`가 적는 워크트리 절대 경로다(노드의 `place.worktree`와 같은 글자).
    Worktree {
        repo: RepoId,
        path: String,
    },
    Node {
        node: NodeId,
    },
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Placed {
    pub spot: Spot,
    pub at: Point,
}

/// 저장한 자리 모두와 정렬한 모양을 지키는 저장소.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Layout {
    pub placed: Vec<Placed>,
    pub arranged: Vec<RepoId>,
}

/// [정렬]이나 정렬한 저장소의 새 자동 배치. `repos`는 정렬한 모양을 지킬 저장소다.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Arranged {
    pub repos: Vec<RepoId>,
    pub placed: Vec<Placed>,
}

/// 사람이 바꾼 표준 카드 크기(그래프 px). 저장한 크기가 없는 카드는 기본 크기다.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CardSize {
    pub node: NodeId,
    pub width: f64,
    pub height: f64,
}

#[derive(Serialize, Deserialize, TS, PartialEq, Debug)]
#[ts(export)]
pub struct Commit {
    pub hash: String,
    pub parents: Vec<String>,
    pub subject: String,
    pub author: String,
    /// 작성 시각, 유닉스 초.
    #[ts(type = "number")]
    pub time: i64,
    pub refs: Vec<GitRef>,
}

#[derive(Serialize, Deserialize, TS, PartialEq, Debug)]
#[serde(tag = "kind", content = "name", rename_all = "camelCase")]
#[ts(export)]
pub enum GitRef {
    /// 로컬·원격 브랜치. 원격은 `origin/main`처럼 원격 이름이 붙는다.
    Branch(String),
    Tag(String),
}

/// 커밋 그래프 칸이 읽는 것: 한 구간의 커밋과, 워크트리마다 체크아웃한 커밋.
#[derive(Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CommitGraph {
    pub commits: Vec<Commit>,
    /// 참조의 지문(`commit_graph::version`). 같으면 이미 읽은 구간도 그대로다.
    pub version: String,
    pub worktrees: Vec<Worktree>,
}
