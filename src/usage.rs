use std::path::Path;
use std::time::Duration;

use serde::Serialize;
use ts_rs::TS;

use crate::protocol::LaunchError;

pub const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Window {
    /// 사용한 비율(0~100). Orca 기본 표시와 같다.
    pub used: u8,
    /// 창이 다시 차는 시각. 유닉스 초.
    #[ts(type = "number | null")]
    pub resets_at: Option<i64>,
}

/// 모델 하나나 모델 묶음에만 걸린 창. 예: 한 모델의 주간 창, 모델 묶음의 결제 주기 창.
#[derive(Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelWindow {
    /// 출처가 준 모델 표시 이름 그대로다.
    pub model: String,
    pub window: Window,
}

#[derive(Clone, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Limits {
    Ready {
        session: Option<Window>,
        weekly: Option<Window>,
        models: Vec<ModelWindow>,
        /// 결제 주기 동안 포함 사용량 전체. 결제 주기가 있는 출처만 준다.
        plan: Option<Window>,
    },
    /// 로그인하지 않았거나 로그인이 끝났다.
    NeedsLogin,
    Failed {
        problem: UsageProblem,
    },
}

/// 사용량을 보일 수 없는 이유. 화면이 고른 언어로 쓴다.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(tag = "code", content = "value", rename_all = "camelCase")]
#[ts(export)]
pub enum UsageProblem {
    /// CLI가 `PATH`에 없다. 그 CLI를 설치하지 않은 기계다.
    NotInstalled(String),
    /// CLI를 실행하지 못했다. `detail`은 OS 오류 글이다.
    CliFailed { command: String, detail: String },
    /// CLI가 알 수 없는 형식을 냈다.
    CliFormat(String),
    /// CLI가 `PROBE_TIMEOUT` 안에 답하지 않았다.
    NoReply(String),
    /// API 키·토큰 로그인은 사용량을 볼 수 없다.
    LoginNotSupported,
    /// 구독 로그인인데 loom이 토큰을 찾지 못했다. 값은 에이전트 이름이다.
    NoToken(String),
    /// 사용량 API가 알 수 없는 형식을 냈다.
    ResponseFormat,
    /// 사용량 API가 요청 한도를 넘었다고 답했다. `retry_at`(유닉스 초)까지 다시 부르지 않는다.
    #[serde(rename_all = "camelCase")]
    RateLimited {
        #[ts(type = "number")]
        retry_at: i64,
    },
    /// 로그인 토큰이 만료됐다. 그 계정으로 CLI를 한 번 실행하면 갱신된다. 값은 에이전트 이름이다.
    TokenExpired(String),
    /// 네트워크 오류 같은 그 밖의 실패. 값은 원인 글이다.
    Unavailable(String),
}

#[derive(Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Usage {
    pub email: Option<String>,
    /// 요금제 이름. CLI가 준 글자 그대로다(예: `max`, `plus`).
    pub plan: Option<String>,
    pub limits: Limits,
    /// 이 값을 읽은 시각. 유닉스 초. 캐시가 앞 값을 다시 주면 그 값을 읽은 때다.
    #[ts(type = "number")]
    pub read_at: i64,
}

impl Usage {
    pub fn new(email: Option<String>, plan: Option<String>, limits: Limits) -> Self {
        Usage {
            email,
            plan,
            limits,
            read_at: jiff::Timestamp::now().as_second(),
        }
    }

    pub fn failed(problem: UsageProblem) -> Self {
        Self::new(None, None, Limits::Failed { problem })
    }
}

pub fn launch_failed(e: LaunchError) -> Usage {
    match e {
        LaunchError::NotInstalled { command, .. } => {
            Usage::failed(UsageProblem::NotInstalled(command))
        }
        e => Usage::failed(UsageProblem::Unavailable(e.to_string())),
    }
}

pub fn fingerprint(path: &Path) -> Option<u64> {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let bytes = std::fs::read(path).ok()?;
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    Some(hasher.finish())
}

pub fn percent(used_percent: f64) -> u8 {
    used_percent.round().clamp(0.0, 100.0) as u8
}

pub fn failed(problem: UsageProblem) -> Limits {
    Limits::Failed { problem }
}

pub fn run_failed(command: &str, e: &std::io::Error) -> UsageProblem {
    if e.kind() == std::io::ErrorKind::NotFound {
        let program = command.split(' ').next().unwrap_or(command);
        return UsageProblem::NotInstalled(program.into());
    }
    UsageProblem::CliFailed {
        command: command.into(),
        detail: e.to_string(),
    }
}
