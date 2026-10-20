//! 사용량: 계정마다 5시간·주간 창의 사용한 비율을 CLI와 같은 출처에서 읽는다. Orca와 같은 출처다.
//! Claude는 로그인 폴더의 OAuth 토큰으로 `api/oauth/usage`를, Codex는 `codex app-server`를 부른다.
//! 바뀌는 이유는 두 CLI와 그 API다. 막는 호출이므로 데몬은 `spawn_blocking`에서 부른다.
//! 근거: docs/design.md 「계정과 사용량」.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::process_tree;
use crate::profile::Provider;

/// CLI나 API 하나를 기다리는 상한. 넘으면 그 계정만 「지금 볼 수 없다」가 된다.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
/// Codex 창 길이가 이 값 이하면 5시간 창이다. Codex는 5시간 창(300분)과 주간 창(10080분)을 primary·secondary 순서 없이 준다.
const SESSION_WINDOW_MAX_MINS: u64 = 300;
const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
/// Orca가 보내는 값과 같다. 기본 User-Agent(`ureq/…`)는 429를 받았다(2026-10-20, docs/design.md 「실측 근거」).
const CLAUDE_USER_AGENT: &str = "claude-code/2.1.0";

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

/// 모델 하나에만 걸린 주간 창. 예: Claude 「Fable」.
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
    /// CLI를 실행하지 못했다. `detail`은 OS 오류 글이다.
    CliFailed { command: String, detail: String },
    /// CLI가 알 수 없는 형식을 냈다.
    CliFormat(String),
    /// CLI가 `PROBE_TIMEOUT` 안에 답하지 않았다.
    NoReply(String),
    /// API 키 같은 로그인 방식은 사용량을 볼 수 없다.
    LoginNotSupported,
    /// 사용량 API가 알 수 없는 형식을 냈다.
    ResponseFormat,
    /// OAuth 토큰이 만료됐다. 그 계정으로 CLI를 한 번 실행하면 갱신된다.
    TokenExpired,
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
}

impl Usage {
    pub fn failed(problem: UsageProblem) -> Self {
        Usage {
            email: None,
            plan: None,
            limits: Limits::Failed { problem },
        }
    }
}

/// `config_dir`가 `None`이면 시스템 기본 로그인을 읽는다.
pub fn probe(provider: Provider, config_dir: Option<&Path>) -> Usage {
    match provider {
        Provider::Claude => claude(config_dir),
        Provider::Codex => codex(config_dir),
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
    let mut cmd = process_tree::command("claude");
    cmd.args(["auth", "status", "--json"]);
    if let Some(dir) = config_dir {
        cmd.env("CLAUDE_CONFIG_DIR", dir);
    }
    // 로그인하지 않았으면 0이 아닌 코드로 끝나도 JSON은 낸다. 코드는 보지 않는다.
    let status: ClaudeStatus = match cmd.stdin(Stdio::null()).stderr(Stdio::null()).output() {
        Ok(out) => match serde_json::from_slice(&out.stdout) {
            Ok(status) => status,
            Err(_) => {
                return Usage::failed(UsageProblem::CliFormat("claude auth status".into()));
            }
        },
        Err(e) => {
            return Usage::failed(UsageProblem::CliFailed {
                command: "claude".into(),
                detail: e.to_string(),
            });
        }
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
        return failed(UsageProblem::LoginNotSupported);
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(PROBE_TIMEOUT))
        .build()
        .into();
    let response = agent
        .get(CLAUDE_USAGE_URL)
        .header("Authorization", format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", CLAUDE_USER_AGENT)
        .call();
    match response {
        Ok(mut res) => match res.body_mut().read_json::<Value>() {
            Ok(body) => Limits::Ready {
                session: claude_window(
                    &body["five_hour"]["utilization"],
                    &body["five_hour"]["resets_at"],
                ),
                weekly: claude_window(
                    &body["seven_day"]["utilization"],
                    &body["seven_day"]["resets_at"],
                ),
                models: claude_models(&body["limits"]),
            },
            Err(_) => failed(UsageProblem::ResponseFormat),
        },
        Err(ureq::Error::StatusCode(401)) => failed(UsageProblem::TokenExpired),
        Err(e) => failed(UsageProblem::Unavailable(e.to_string())),
    }
}

/// `used`는 사용한 %, `resets_at`은 ISO 8601 시각이다.
fn claude_window(used: &Value, resets_at: &Value) -> Option<Window> {
    Some(Window {
        used: percent(used.as_f64()?),
        resets_at: resets_at
            .as_str()
            .and_then(|s| s.parse::<jiff::Timestamp>().ok())
            .map(|t| t.as_second()),
    })
}

/// `limits` 가운데 모델 하나에 걸린 주간 창: `{"kind": "weekly_scoped", "percent": 사용한 %, "resets_at", "scope": {"model": {"display_name"}}}`.
fn claude_models(limits: &Value) -> Vec<ModelWindow> {
    let Some(items) = limits.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter(|v| v["kind"] == "weekly_scoped")
        .filter_map(|v| {
            Some(ModelWindow {
                model: v["scope"]["model"]["display_name"].as_str()?.to_owned(),
                window: claude_window(&v["percent"], &v["resets_at"])?,
            })
        })
        .collect()
}

fn codex(home: Option<&Path>) -> Usage {
    let replies = match app_server(home) {
        Ok(replies) => replies,
        Err(problem) => return Usage::failed(problem),
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
        return failed(UsageProblem::LoginNotSupported);
    }
    let (mut session, mut weekly) = (None, None);
    for w in [&rate_limits["primary"], &rate_limits["secondary"]] {
        let (Some(used), Some(mins)) =
            (w["usedPercent"].as_f64(), w["windowDurationMins"].as_u64())
        else {
            continue;
        };
        let window = Some(Window {
            used: percent(used),
            resets_at: w["resetsAt"].as_i64(),
        });
        if mins <= SESSION_WINDOW_MAX_MINS {
            session = window;
        } else {
            weekly = window;
        }
    }
    Limits::Ready {
        session,
        weekly,
        models: Vec::new(),
    }
}

/// `account/read`와 `account/rateLimits/read`의 결과를 순서대로 돌려준다.
fn app_server(home: Option<&Path>) -> Result<[Value; 2], UsageProblem> {
    let mut cmd = process_tree::command("codex");
    cmd.arg("app-server");
    if let Some(dir) = home {
        cmd.env("CODEX_HOME", dir);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| app_server_failed(&e))?;
    let result = exchange(&mut child);
    // 서버는 stdin이 열려 있는 동안 돈다. 답을 받았으면 끝낸다.
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn exchange(child: &mut std::process::Child) -> Result<[Value; 2], UsageProblem> {
    let requests = [
        r#"{"method":"initialize","id":0,"params":{"clientInfo":{"name":"loom","version":"0"}}}"#,
        r#"{"method":"initialized"}"#,
        r#"{"method":"account/read","id":1,"params":{"refreshToken":false}}"#,
        r#"{"method":"account/rateLimits/read","id":2}"#,
    ];
    let mut stdin = child.stdin.take().expect("stdin을 piped로 열었다");
    for line in requests {
        writeln!(stdin, "{line}").map_err(|e| app_server_failed(&e))?;
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
            .map_err(|_| UsageProblem::NoReply(APP_SERVER.into()))?;
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

fn percent(used_percent: f64) -> u8 {
    used_percent.round().clamp(0.0, 100.0) as u8
}

fn failed(problem: UsageProblem) -> Limits {
    Limits::Failed { problem }
}

const APP_SERVER: &str = "codex app-server";

fn app_server_failed(e: &std::io::Error) -> UsageProblem {
    UsageProblem::CliFailed {
        command: APP_SERVER.into(),
        detail: e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ready(limits: Limits) -> (Option<Window>, Option<Window>) {
        match limits {
            Limits::Ready {
                session, weekly, ..
            } => (session, weekly),
            _ => panic!("Ready가 아니다"),
        }
    }

    #[test]
    fn claude_window_reads_used_percent_and_iso_time() {
        let w = claude_window(&json!(45.0), &json!("2026-10-19T11:29:59.565320+00:00")).unwrap();
        assert_eq!(w.used, 45);
        assert_eq!(w.resets_at, Some(1792409399));
        assert!(claude_window(&Value::Null, &Value::Null).is_none());
    }

    #[test]
    fn claude_models_read_weekly_scoped_limits() {
        let models = claude_models(&json!([
            {"kind": "session", "percent": 54, "resets_at": "2026-10-20T02:29:59Z", "scope": null},
            {"kind": "weekly_scoped", "percent": 12, "resets_at": "2026-10-24T12:00:00+00:00",
             "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null}},
            {"kind": "weekly_scoped", "percent": 5, "scope": {"model": null, "surface": "cowork"}},
        ]));
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].model, "Fable");
        assert_eq!(models[0].window.used, 12);
        assert_eq!(models[0].window.resets_at, Some(1792843200));
        assert!(claude_models(&Value::Null).is_empty());
    }

    #[test]
    fn codex_windows_are_sorted_by_length_not_position() {
        let (session, weekly) = ready(codex_limits(&json!({
            "primary": {"usedPercent": 12, "windowDurationMins": 10080, "resetsAt": 1792905010},
            "secondary": null,
        })));
        assert!(session.is_none());
        assert_eq!(weekly.unwrap().used, 12);
        let (session, weekly) = ready(codex_limits(&json!({
            "primary": {"usedPercent": 81, "windowDurationMins": 300, "resetsAt": 1},
            "secondary": {"usedPercent": 0, "windowDurationMins": 10080, "resetsAt": 2},
        })));
        assert_eq!(session.unwrap().used, 81);
        assert_eq!(weekly.unwrap().used, 0);
    }
}
