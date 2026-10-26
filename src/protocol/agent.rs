use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 카탈로그의 `id`. 예: `claude`, `gemini`.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentId(pub String);

/// 계정과 사용량이 있는 공급자. 근거: docs/design/에이전트-실행.md 「계정과 사용량」.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Provider {
    Claude,
    Codex,
}

#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct Model {
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct ModelChoice {
    pub model: Option<String>,
    pub effort: Option<String>,
}

impl ModelChoice {
    pub fn is_empty(&self) -> bool {
        self.model.is_none() && self.effort.is_none()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ServerLink {
    Codex {
        url: String,
        pid: Option<u32>,
    },
    OpenCode2 {
        url: String,
        pid: Option<u32>,
        password: String,
    },
}

/// 노드를 띄우지 못한 이유. 에이전트에게는 한국어 문장(`Display`)이, 화면에는 코드와 값(JSON)이 간다.
#[derive(Debug, Serialize, TS)]
#[serde(tag = "code", content = "value", rename_all = "camelCase")]
#[ts(export)]
pub enum LaunchError {
    UnknownAgent(AgentId),
    NotInstalled {
        agent: String,
        command: String,
        /// WSL 노드면 찾은 배포판이다. 없으면 데몬 PATH에서 찾았다.
        distro: Option<String>,
    },
    BriefTooLong {
        #[ts(type = "number")]
        len: usize,
        #[ts(type = "number")]
        limit: usize,
    },
    /// 노드가 붙을 서버(`codex app-server`, `opencode serve`)를 띄우지 못했다. `cause`는 OS·서버 오류 글이다.
    Server {
        program: String,
        cause: String,
    },
    /// 서버를 쓰는 프로필인데 서버가 없다. 값은 에이전트 이름이다.
    NoServer(String),
    /// 첫 지시문을 받지 않는 에이전트다. 값은 에이전트 이름이다.
    NoBrief(String),
    /// 모델·추론 강도를 시작 인자로 받지 않는 에이전트다. 값은 에이전트 이름이다.
    NoModelChoice(String),
    /// WSL 폴더의 노드인데 그 배포판을 쓸 수 없다.
    Wsl(InsideError),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LaunchError::UnknownAgent(id) => {
                write!(f, "{}: this agent is not in the catalog.", id.0)
            }
            LaunchError::NotInstalled {
                agent,
                command,
                distro: None,
            } => write!(
                f,
                "{agent}: the daemon PATH has no `{command}`. Install it, then click Find again in settings."
            ),
            LaunchError::NotInstalled {
                agent,
                command,
                distro: Some(distro),
            } => write!(
                f,
                "{agent}: could not find `{command}` in WSL · {distro}. Install it in that distro, then click Find again in settings."
            ),
            LaunchError::BriefTooLong { len, limit } => write!(
                f,
                "The brief is {len} bytes. Make it {limit} bytes or less, and put long material in a file and send its path."
            ),
            LaunchError::Server { program, cause } => {
                write!(f, "Could not start `{program}`: {cause}")
            }
            LaunchError::NoServer(agent) => {
                write!(f, "{agent}: could not start the server for the node.")
            }
            LaunchError::NoModelChoice(agent) => write!(
                f,
                "{agent}: does not take this `--model` or `--effort` (Claude, Codex and Pi take both, Cursor takes `--model` with an optional `--effort`). Fix them and try again."
            ),
            LaunchError::Wsl(e) => e.fmt(f),
            LaunchError::NoBrief(agent) => write!(
                f,
                "{agent}: does not take a brief. Start it without a brief and type into its terminal."
            ),
        }
    }
}

pub type StartDigests = BTreeMap<String, String>;

/// 배포판을 쓸 수 없다. 에이전트에게는 `Display` 글이, 화면에는 코드와 값이 간다.
#[derive(Debug, PartialEq, serde::Serialize, ts_rs::TS)]
#[serde(tag = "code", rename_all = "camelCase")]
#[ts(export)]
pub enum InsideError {
    /// 배포판이 시작하지 못했다. `cause`는 배포판 호스트(`wsl.exe`)나 배포판 안 명령의 오류 글이다.
    Start { distro: String, cause: String },
    /// 배포판 안에서 Windows 프로그램을 띄울 수 없다(`/etc/wsl.conf` `[interop] enabled=false`). 노드의 `loom` 명령이 닿지 않는다.
    Interop { distro: String },
}

impl std::fmt::Display for InsideError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InsideError::Start { distro, cause } => write!(
                f,
                "Could not start {distro}: {cause}. Run `wsl -d {distro}` in a terminal to see the cause."
            ),
            InsideError::Interop { distro } => write!(
                f,
                "{distro} cannot run Windows programs (WSL interop is off), so the node could not use loom commands. Set [interop] enabled=true in /etc/wsl.conf, run `wsl --shutdown`, and try again."
            ),
        }
    }
}
