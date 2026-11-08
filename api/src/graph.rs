pub mod docs;
pub mod errors;
pub mod events;
pub mod messages;
pub mod pty;
pub mod search;
pub mod text;
pub mod waits;

#[cfg(feature = "axum")]
mod response;
#[cfg(feature = "sqlite")]
mod sqlite;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ids::{AgentId, NodeId, RepoId};
use crate::start::Start;
use events::EventRecord;
use waits::ScreenWait;

/// 노드 일의 상태(일 축).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum NodeState {
    Running,
    Done,
    /// `loom spawn --worktree`의 설정 스크립트가 이 코드로 끝나 에이전트를 띄우지 않았다.
    SetupFailed {
        code: u32,
    },
}

/// 노드 프로세스의 상태(프로세스 축).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum ProcessState {
    /// 프로세스가 턴 중이거나 막 떴다.
    Working,
    /// 턴이 끝났다.
    Idle,
    /// 데몬이 완료 노드의 프로세스를 재웠다. 메시지가 오면 다시 띄운다.
    Asleep,
    /// 사람의 답을 기다리는 창이 에이전트를 막았다. `before`는 창이 닫히면 돌아갈 상태다(`working`이나 `idle`).
    Blocked { before: Box<ProcessState> },
    /// 프로세스가 없다. `code`가 없으면 끝 코드를 모른다: 데몬이 다시 시작했을 때 사라졌거나, 호스트가 없거나, 에이전트를 띄우지 않았다.
    Stopped { code: Option<u32> },
}

/// 카드의 상태 칩과 그 아래 줄. 데몬이 노드를 읽을 때 이미 가진 사실에서 정한다(2026-11-08 목업 docs/mockups/agent-state.html M77 승인, Q2669).
/// 한 노드에 여럿이 맞으면 사람 → 문제 → 일함 → 대기 → 결과 차례로 앞의 것이다.
#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Status {
    /// 사람의 답·승인을 기다린다. 글과 시각은 `waits`의 가장 오랜 `ask` 대기가 준다.
    Human,
    /// `NodeState::SetupFailed`와 같다.
    SetupFailed { code: u32 },
    /// 일이 남았는데 프로세스가 없다. `ProcessState::Stopped`와 같다.
    Stopped { code: Option<u32> },
    /// 사용 한도에 걸렸다. 계정을 바꾸면 풀린다.
    Limited,
    /// 턴 중이다. `tool`은 마지막으로 부른 도구와 대상(`Edit · web/src/NodeCard.tsx`)이다. 도구를 알리지 않는 에이전트이거나 아직 부르지 않았으면 없다.
    Working { tool: Option<String> },
    /// 끝나지 않은 산 자식이 있다. 자식 이름(만든 차례).
    Children { names: Vec<String> },
    /// 내 브랜치의 관문 차례가 열렸다(`ready`). `judged`는 이 head에 판정한 리뷰어 수다.
    Review {
        branch: String,
        judged: u32,
        reviewers: u32,
    },
    /// 판정이 통과했고 병합 큐에 있다(`passed`, `merging`). 병합 큐가 실패해 루트의 결정을 기다릴 때(`failed`)도 같다.
    Merge { branch: String },
    /// 부모에게 `loom ask`로 물었고 답이 없다.
    Answer,
    /// 리뷰어로서 이 head에 이미 판정했다. 새 커밋을 기다린다.
    Rereview { branch: String },
    /// 턴 끝 글이 「⏳ 」로 시작한다(loom 규칙: 기다릴 때 쓰는 글). 글은 `reply`가 준다.
    Job,
    /// 이 노드가 루트인 배포가 끝나지 않았다.
    Deploy { target: String, sha: String },
    /// 받을 메시지나 다시 띄우기 계획이 있다. 곧 일한다.
    Mail,
    /// 쉼. 감시하지 않는 노드(명령 노드, 듣지 못함)이거나 아직 턴을 끝낸 적이 없는 노드(빈 지시문으로 띄운 루트)다.
    #[default]
    Idle,
    /// `loom done` 뒤다.
    Done,
    /// 내 브랜치가 병합됐다.
    Merged { target: String, sha: String },
}

/// 카드 보기. `Standard`는 카드에 터미널이 있다. `Simple`은 카드에 머리 정보만 있고, 고른 노드의 터미널은 오른쪽 칸에만 열린다.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CardView {
    Standard,
    Simple,
}

/// 노드가 도는 자리: 저장소와 그 워크트리(묶음). `worktree`는 `git worktree list`가 적는 절대 경로다.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Place {
    pub repo: RepoId,
    pub worktree: String,
}

#[derive(Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NodeView {
    pub id: NodeId,
    pub name: String,
    /// 카탈로그 에이전트. 명령(기본 셸 포함)이면 없다.
    pub agent: Option<AgentId>,
    pub parent: Option<NodeId>,
    pub state: NodeState,
    pub process: ProcessState,
    /// 아직 꺼내지 않은 메시지 수(모든 방식).
    pub unread: u32,
    /// CLI가 알린 모델 id 그대로다. 명령 노드이거나 CLI가 알리지 않았으면 없다.
    pub model: Option<String>,
    /// CLI가 알린 추론 강도 그대로다. 명령 노드이거나 CLI가 알리지 않았으면 없다.
    pub effort: Option<String>,
    pub place: Place,
    /// 사이드바 북마크 영역의 차례. 작을수록 먼저 북마크했다. 북마크하지 않았으면 없다.
    pub bookmark: Option<u32>,
    /// 사람이 「⋯」 메뉴로 이 노드에 고른 카드 보기. 없으면 설정의 기본 보기를 따른다.
    pub card_view: Option<CardView>,
    /// 마지막 턴을 끝낸 답 글. 심플 카드의 상태 줄이다(목업 docs/mockups/node-card-view.html ⑧). 답 글을 받은 적이 없으면 없다.
    pub reply: Option<String>,
    /// 사람의 답·승인을 기다린다. 열린 `loom ask` 질문들(물은 차례), 그 뒤에 신호의 대기 하나. 없으면 빈 목록이다.
    /// 근거: docs/design/메시징.md 「사람 응답 대기」.
    pub waits: Vec<ScreenWait>,
    /// 노드 메뉴의 다시 띄우기. 없으면 메뉴에 보이지 않는다.
    pub relaunch: Option<RelaunchOffer>,
    /// 카드의 상태 칩과 아래 줄. 이 필드가 없는 옛 데몬의 노드는 `idle`로 읽는다.
    #[serde(default)]
    pub status: Status,
    /// `status`가 시작된 유닉스 초. 아래 줄의 시간이다. 옛 데몬이면 0이다.
    #[serde(default)]
    #[ts(type = "number")]
    pub since: i64,
    /// 내 브랜치의 지금 관문에서 받은 판정 막음 수. 3 이상이면 칩 옆에 보인다. 옛 데몬이면 0이다.
    #[serde(default)]
    pub rework: u32,
}

/// 노드 메뉴가 보이는 다시 띄우기. 데몬이 읽을 때 노드 상태, 에이전트의 메시지 연동, 남은 세션, 다시 띄우기 계획에서 정한다.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RelaunchOffer {
    Ready,
    /// 계획이 있다. 턴 끝을 기다리거나 다시 띄우는 중이다.
    Queued,
}

#[derive(Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum GraphEvent {
    /// 노드나 그 대기가 바뀌었다. 값은 담지 않는다. 화면은 `GET /api/nodes`로 노드 목록을 다시 읽는다. 연결하면 먼저 한 번 보낸다.
    Nodes,
    /// `child`와 그 부모 사이 간선으로 메시지가 `toward` 쪽으로 갔다.
    Message { child: NodeId, toward: Toward },
    /// 부모·자식이 아닌 두 노드 사이로 메시지가 `from`에서 `to`로 갔다. 둘 사이에는 간선이 없다.
    Hop { from: NodeId, to: NodeId },
    /// 이벤트 기록에 한 줄을 더했다.
    Recorded { record: Box<EventRecord> },
    /// 설정, 카드 자리, 카드 크기, 업데이트 상태 가운데 하나가 바뀌었거나 짝짓기 코드가 닫혔거나(짝지음, 다섯 번 틀림) 로그인 창의 명령이 끝났다. 값은 담지 않는다. 화면은 보고 있는 것을 다시 읽는다.
    ViewChanged,
    /// 데몬이 연결이 살아 있음을 알린다. 첫 `Nodes` 바로 뒤, `Pace`를 받은 바로 뒤, 다른 이벤트를 보내지 않은 동안 화면이 정한 간격(`Pace`)마다 보낸다.
    /// 화면은 `quietMs` 동안 아무 프레임도 받지 못하면 연결이 죽었다고 보고 그 연결만 다시 연다.
    Beat {
        #[serde(rename = "quietMs")]
        quiet_ms: u32,
    },
}

/// 간선 위 메시지가 가는 쪽.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Toward {
    Parent,
    Child,
}

#[derive(Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NewNode {
    pub start: Start,
    pub place: Place,
}

/// 노드 이름 바꾸기. 이름 칸을 두 번 누르거나 F2를 눌러 고친다.
#[derive(Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RenameNode {
    pub name: String,
}
