use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ids::AgentId;
use crate::repos::PlaceView;

/// 계정과 사용량이 있는 공급자. 근거: docs/design/에이전트-실행.md 「계정과 사용량」.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Provider {
    Claude,
    Codex,
}

/// 메시지 연동(훅·app-server·서버 API)이 있는 에이전트. 근거: docs/design/에이전트-실행.md.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Profile {
    Claude,
    #[serde(rename = "openclaude")]
    OpenClaude,
    QwenCode,
    Codex,
    Copilot,
    Trae,
    #[serde(rename = "opencode2")]
    OpenCode2,
    #[serde(rename = "mimo-code")]
    MiMoCode,
    Kilo,
    Cursor,
    Pi,
    Omp,
    Dsh,
    Antigravity,
    Crush,
    Hermes,
    Vibe,
}

/// 설정 화면과 새 노드 창이 보는 카탈로그 한 줄. 메시지 연동이 있는 에이전트만 보인다.
#[derive(Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentView {
    pub id: AgentId,
    pub label: String,
    pub homepage: String,
    pub profile: Profile,
    /// 계정을 고르는 공급자. 프로필이 있어도 없을 수 있다(OpenCode 2).
    pub provider: Option<Provider>,
    /// 모든 권한 허용 인자가 있다. 없으면 설정에서 끄고 켤 수 없다. 위치와 관계없이 하나다.
    pub can_bypass: bool,
    pub model_rule: ModelRule,
    /// 서버로 돈다(app-server, 서버 API). loom은 SSH 호스트의 서버에 아직 닿지 못한다.
    pub serves: bool,
    /// 위치마다 찾은 결과. 첫째가 이 PC, 그 뒤가 WSL 배포판, 마지막이 SSH 호스트다. 모든 에이전트가 같은 위치를 같은 순서로 갖는다.
    pub places: Vec<AgentAt>,
}

/// 모델과 추론 강도를 고르는 규칙.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ModelRule {
    Fixed,
    Free,
    EffortNeedsModel,
}

/// 한 위치(이 PC, WSL 배포판, SSH 호스트)에서 에이전트를 찾은 결과.
#[derive(Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentAt {
    pub place: PlaceView,
    /// 그 위치의 `PATH`에서 찾은 실행 파일. 없으면 그 위치에 설치하지 않은 것이다.
    pub path: Option<String>,
    /// 메시지 연동이 그 CLI 버전과 맞지 않을 수 있다는 경고.
    pub version_warning: Option<VersionMismatch>,
}

/// 설치한 CLI 버전이 메시지 연동을 만든 고정 버전과 다르다. 화면이 프로필마다 경고 문장으로 바꾼다.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct VersionMismatch {
    pub installed: String,
    pub pinned: String,
}
