use serde::{Deserialize, Serialize};
use ts_rs::TS;

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
