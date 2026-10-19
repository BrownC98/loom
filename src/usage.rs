//! 사용량: 계정마다 5시간·주간 창의 남은 비율을 CLI와 같은 출처에서 읽는다. Orca와 같은 출처다.
//! Claude는 로그인 폴더의 OAuth 토큰으로 `api/oauth/usage`를, Codex는 `codex app-server`를 부른다.
//! 바뀌는 이유는 두 CLI와 그 API다. 막는 호출이므로 데몬은 `spawn_blocking`에서 부른다.
//! 근거: docs/design.md 「계정과 사용량」.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::profile::Profile;

/// CLI나 API 하나를 기다리는 상한. 넘으면 그 계정만 「지금 볼 수 없다」가 된다.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
/// Codex 창 길이가 이 값 이하면 5시간 창이다. Codex는 5시간 창(300분)과 주간 창(10080분)을 primary·secondary 순서 없이 준다.
const SESSION_WINDOW_MAX_MINS: u64 = 300;
const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

#[derive(Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Window {
    /// 남은 비율(0~100).
    pub remaining: u8,
    /// 창이 다시 차는 시각. 유닉스 초.
    #[ts(type = "number | null")]
    pub resets_at: Option<i64>,
}

#[derive(Clone, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Limits {
    Ready {
        session: Option<Window>,
        weekly: Option<Window>,
    },
    /// 로그인하지 않았거나 로그인이 끝났다.
    NeedsLogin,
    /// 문장은 그대로 화면에 간다.
    Failed { message: String },
}

#[derive(Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Usage {
    pub email: Option<String>,
    /// 요금제 이름. CLI가 준 글자 그대로다(예: `max`, `plus`).
    pub plan: Option<String>,
    pub limits: Limits,
}

impl Usage {
    pub fn failed(message: impl Into<String>) -> Self {
        Usage {
            email: None,
            plan: None,
            limits: Limits::Failed {
                message: message.into(),
            },
        }
    }
}

/// `config_dir`가 `None`이면 시스템 기본 로그인을 읽는다.
pub fn probe(provider: Profile, config_dir: Option<&Path>) -> Usage {
    match provider {
        Profile::Claude => claude(config_dir),
        Profile::Codex => codex(config_dir),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeStatus {
    logged_in: bool,
    email: Option<String>,
    subscription_type: Option<String>,
    config_directory: PathBuf,
}

fn claude(config_dir: Option<&Path>) -> Usage {
    let mut cmd = Command::new("claude");
    cmd.args(["auth", "status", "--json"]);
    if let Some(dir) = config_dir {
        cmd.env("CLAUDE_CONFIG_DIR", dir);
    }
    // 로그인하지 않았으면 0이 아닌 코드로 끝나도 JSON은 낸다. 코드는 보지 않는다.
    let status: ClaudeStatus = match cmd.stdin(Stdio::null()).stderr(Stdio::null()).output() {
        Ok(out) => match serde_json::from_slice(&out.stdout) {
            Ok(status) => status,
            Err(_) => return Usage::failed("`claude auth status`가 알 수 없는 형식을 냈다."),
        },
        Err(e) => return Usage::failed(format!("`claude`를 실행하지 못했다: {e}")),
    };
    let limits = if status.logged_in {
        claude_limits(&status.config_directory)
    } else {
        Limits::NeedsLogin
    };
    Usage {
        email: status.email,
        plan: status.subscription_type,
        limits,
    }
}

fn claude_limits(config_dir: &Path) -> Limits {
    let token = std::fs::read(config_dir.join(".credentials.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|v| {
            v["claudeAiOauth"]["accessToken"]
                .as_str()
                .map(str::to_owned)
        });
    let Some(token) = token else {
        return Limits::Failed {
            message: "이 로그인 방식(API 키, macOS 키체인)은 사용량을 볼 수 없다.".into(),
        };
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(PROBE_TIMEOUT))
        .build()
        .into();
    let response = agent
        .get(CLAUDE_USAGE_URL)
        .header("Authorization", format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .call();
    match response {
        Ok(mut res) => match res.body_mut().read_json::<Value>() {
            Ok(body) => Limits::Ready {
                session: claude_window(&body["five_hour"]),
                weekly: claude_window(&body["seven_day"]),
            },
            Err(_) => failed("사용량 응답이 알 수 없는 형식이다."),
        },
        Err(ureq::Error::StatusCode(401)) => {
            failed("토큰이 만료됐다. 이 계정으로 Claude를 한 번 실행하면 갱신된다.")
        }
        Err(e) => failed(format!("사용량을 지금 볼 수 없다: {e}")),
    }
}

/// `{"utilization": 사용한 %, "resets_at": ISO 8601}`.
fn claude_window(v: &Value) -> Option<Window> {
    Some(Window {
        remaining: remaining(v["utilization"].as_f64()?),
        resets_at: v["resets_at"]
            .as_str()
            .and_then(|s| s.parse::<jiff::Timestamp>().ok())
            .map(|t| t.as_second()),
    })
}

fn codex(home: Option<&Path>) -> Usage {
    let replies = match app_server(home) {
        Ok(replies) => replies,
        Err(e) => return Usage::failed(format!("`codex app-server`를 부르지 못했다: {e}")),
    };
    let [account, limits] = replies;
    let account = &account["account"];
    if account.is_null() {
        return Usage {
            email: None,
            plan: None,
            limits: Limits::NeedsLogin,
        };
    }
    let text = |v: &Value| v.as_str().map(str::to_owned);
    Usage {
        email: text(&account["email"]),
        plan: text(&account["planType"]),
        limits: codex_limits(&limits["rateLimits"]),
    }
}

/// `primary`·`secondary` 가운데 창 길이로 5시간 창과 주간 창을 고른다.
fn codex_limits(rate_limits: &Value) -> Limits {
    if rate_limits.is_null() {
        return failed("이 로그인 방식(API 키)은 사용량을 볼 수 없다.");
    }
    let (mut session, mut weekly) = (None, None);
    for w in [&rate_limits["primary"], &rate_limits["secondary"]] {
        let (Some(used), Some(mins)) =
            (w["usedPercent"].as_f64(), w["windowDurationMins"].as_u64())
        else {
            continue;
        };
        let window = Some(Window {
            remaining: remaining(used),
            resets_at: w["resetsAt"].as_i64(),
        });
        if mins <= SESSION_WINDOW_MAX_MINS {
            session = window;
        } else {
            weekly = window;
        }
    }
    Limits::Ready { session, weekly }
}

/// `account/read`와 `account/rateLimits/read`의 결과를 순서대로 돌려준다.
fn app_server(home: Option<&Path>) -> Result<[Value; 2], String> {
    let mut cmd = Command::new("codex");
    cmd.arg("app-server");
    if let Some(dir) = home {
        cmd.env("CODEX_HOME", dir);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let result = exchange(&mut child);
    // 서버는 stdin이 열려 있는 동안 돈다. 답을 받았으면 끝낸다.
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn exchange(child: &mut std::process::Child) -> Result<[Value; 2], String> {
    let requests = [
        r#"{"method":"initialize","id":0,"params":{"clientInfo":{"name":"loom","version":"0"}}}"#,
        r#"{"method":"initialized"}"#,
        r#"{"method":"account/read","id":1,"params":{"refreshToken":false}}"#,
        r#"{"method":"account/rateLimits/read","id":2}"#,
    ];
    let mut stdin = child.stdin.take().expect("stdin을 piped로 열었다");
    for line in requests {
        writeln!(stdin, "{line}").map_err(|e| e.to_string())?;
    }
    let stdout = child.stdout.take().expect("stdout을 piped로 열었다");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                return;
            }
        }
    });
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let mut replies = [Value::Null, Value::Null];
    let mut pending = 2;
    while pending > 0 {
        let line = rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| "응답이 없다".to_owned())?;
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let slot = match msg["id"].as_u64() {
            Some(1) => 0,
            Some(2) => 1,
            _ => continue,
        };
        // 오류 응답이면 빈 결과로 둔다. 한도를 읽지 못해도 계정은 보인다.
        replies[slot] = msg.get("result").cloned().unwrap_or(Value::Null);
        pending -= 1;
    }
    Ok(replies)
}

fn remaining(used_percent: f64) -> u8 {
    (100.0 - used_percent).round().clamp(0.0, 100.0) as u8
}

fn failed(message: impl Into<String>) -> Limits {
    Limits::Failed {
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ready(limits: Limits) -> (Option<Window>, Option<Window>) {
        match limits {
            Limits::Ready { session, weekly } => (session, weekly),
            _ => panic!("Ready가 아니다"),
        }
    }

    #[test]
    fn claude_window_reads_used_percent_and_iso_time() {
        let w = claude_window(
            &json!({"utilization": 45.0, "resets_at": "2026-10-19T11:29:59.565320+00:00"}),
        )
        .unwrap();
        assert_eq!(w.remaining, 55);
        assert_eq!(w.resets_at, Some(1792409399));
        assert!(claude_window(&Value::Null).is_none());
    }

    #[test]
    fn codex_windows_are_sorted_by_length_not_position() {
        let (session, weekly) = ready(codex_limits(&json!({
            "primary": {"usedPercent": 12, "windowDurationMins": 10080, "resetsAt": 1792905010},
            "secondary": null,
        })));
        assert!(session.is_none());
        assert_eq!(weekly.unwrap().remaining, 88);
        let (session, weekly) = ready(codex_limits(&json!({
            "primary": {"usedPercent": 81, "windowDurationMins": 300, "resetsAt": 1},
            "secondary": {"usedPercent": 0, "windowDurationMins": 10080, "resetsAt": 2},
        })));
        assert_eq!(session.unwrap().remaining, 19);
        assert_eq!(weekly.unwrap().remaining, 100);
    }
}
