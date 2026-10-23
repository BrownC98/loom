use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::accounts::{AccountId, Site};
use crate::profile::{LaunchError, Provider};

const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const SESSION_WINDOW_MAX_MINS: u64 = 300;
const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const CLAUDE_USER_AGENT: &str = "claude-code/2.1.0";
const CURSOR_ORIGIN: &str = "https://cursor.com";
const CURSOR_USAGE_URL: &str = "https://cursor.com/api/usage-summary";
const CURSOR_COMMAND: &str = "cursor-agent";
const COOKIE_KEEPS: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');
const CURSOR_MODELS: &str = "Cursor Models";
const CURSOR_OTHER_MODELS: &str = "Other Models";
const CURSOR_ON_DEMAND: &str = "On-demand";

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

/// 모델 하나나 모델 묶음에만 걸린 창. 예: Claude 「Fable」(주간), Cursor 「Cursor Models」(결제 주기).
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
        /// 결제 주기 동안 포함 사용량 전체. Cursor만 준다.
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
    /// API 키 같은 로그인 방식은 사용량을 볼 수 없다.
    LoginNotSupported,
    /// 사용량 API가 알 수 없는 형식을 냈다.
    ResponseFormat,
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

pub fn probe(provider: Provider, site: &Site, id: Option<&AccountId>) -> Usage {
    let cmd = match provider {
        Provider::Claude => site.command(provider, &["auth", "status", "--json"], id),
        Provider::Codex => site.command(provider, &["app-server"], id),
    };
    let cmd = match cmd {
        Ok(cmd) => cmd,
        Err(LaunchError::NotInstalled { command, .. }) => {
            return Usage::failed(UsageProblem::NotInstalled(command));
        }
        Err(e) => return Usage::failed(UsageProblem::Unavailable(e.to_string())),
    };
    match provider {
        Provider::Claude => claude(cmd, site, id),
        Provider::Codex => codex(cmd),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeStatus {
    logged_in: bool,
    email: Option<String>,
    subscription_type: Option<String>,
    config_directory: Option<String>,
}

fn claude(mut cmd: Command, site: &Site, id: Option<&AccountId>) -> Usage {
    let status: ClaudeStatus = match cmd.stdin(Stdio::null()).stderr(Stdio::null()).output() {
        Ok(out) => match serde_json::from_slice(&out.stdout) {
            Ok(status) => status,
            Err(_) => {
                return Usage::failed(UsageProblem::CliFormat("claude auth status".into()));
            }
        },
        Err(e) => return Usage::failed(run_failed("claude", &e)),
    };
    let dir = status
        .config_directory
        .map(|dir| site.local(&dir))
        .or_else(|| site.login_dir(Provider::Claude, id));
    let limits = if !status.logged_in {
        Limits::NeedsLogin
    } else if let Some(dir) = dir {
        claude_limits(&dir)
    } else {
        failed(UsageProblem::CliFormat("claude auth status".into()))
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
                plan: None,
            },
            Err(_) => failed(UsageProblem::ResponseFormat),
        },
        Err(ureq::Error::StatusCode(401)) => failed(UsageProblem::TokenExpired("Claude".into())),
        Err(e) => failed(UsageProblem::Unavailable(e.to_string())),
    }
}

fn claude_window(used: &Value, resets_at: &Value) -> Option<Window> {
    Some(Window {
        used: percent(used.as_f64()?),
        resets_at: resets_at
            .as_str()
            .and_then(|s| s.parse::<jiff::Timestamp>().ok())
            .map(|t| t.as_second()),
    })
}

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

fn codex(cmd: Command) -> Usage {
    let replies = match app_server(cmd) {
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
        plan: None,
    }
}

fn app_server(mut cmd: Command) -> Result<[Value; 2], UsageProblem> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| app_server_failed(&e))?;
    let result = exchange(&mut child);
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
        replies[slot] = msg.get("result").cloned().unwrap_or(Value::Null);
        pending -= 1;
    }
    Ok(replies)
}

pub fn probe_cursor(site: &Site) -> Usage {
    if !site.installed(CURSOR_COMMAND) {
        return Usage::failed(UsageProblem::NotInstalled(CURSOR_COMMAND.into()));
    }
    let read = |rel: &str| {
        site.home_file(rel)
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    };
    let email = read(".cursor/cli-config.json")
        .and_then(|v| v["authInfo"]["email"].as_str().map(str::to_owned));
    let token = read(".config/cursor/auth.json")
        .and_then(|v| v["accessToken"].as_str().and_then(CursorToken::parse));
    let limits = match token {
        Some(token) => cursor_limits(&token),
        None if email.is_some() => failed(UsageProblem::LoginNotSupported),
        None => Limits::NeedsLogin,
    };
    Usage {
        email,
        plan: None,
        limits,
    }
}

struct CursorToken {
    raw: String,
    subject: String,
    expires_at: Option<i64>,
}

impl CursorToken {
    fn parse(raw: &str) -> Option<Self> {
        use base64::Engine;
        let body = raw.split('.').nth(1)?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(body.trim_end_matches('='))
            .ok()?;
        let claims: Value = serde_json::from_slice(&bytes).ok()?;
        Some(Self {
            raw: raw.to_owned(),
            subject: claims["sub"].as_str().filter(|s| !s.is_empty())?.to_owned(),
            expires_at: claims["exp"].as_i64(),
        })
    }

    fn cookie(&self) -> String {
        let subject = utf8_percent_encode(&self.subject, COOKIE_KEEPS);
        format!("WorkosCursorSessionToken={subject}%3A%3A{}", self.raw)
    }
}

fn cursor_limits(token: &CursorToken) -> Limits {
    let expired = || failed(UsageProblem::TokenExpired("Cursor".into()));
    if token
        .expires_at
        .is_some_and(|exp| exp <= jiff::Timestamp::now().as_second())
    {
        return expired();
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(PROBE_TIMEOUT))
        .build()
        .into();
    let response = agent
        .get(CURSOR_USAGE_URL)
        .header("Cookie", token.cookie())
        .header("Accept", "application/json")
        .header("Origin", CURSOR_ORIGIN)
        .header("Referer", format!("{CURSOR_ORIGIN}/dashboard"))
        .call();
    match response {
        Ok(mut res) => match res.body_mut().read_json::<Value>() {
            Ok(body) => cursor_summary(&body),
            Err(_) => failed(UsageProblem::ResponseFormat),
        },
        Err(ureq::Error::StatusCode(401)) => expired(),
        Err(e) => failed(UsageProblem::Unavailable(e.to_string())),
    }
}

fn cursor_summary(body: &Value) -> Limits {
    let resets_at = body["billingCycleEnd"]
        .as_str()
        .and_then(|s| s.parse::<jiff::Timestamp>().ok())
        .map(|t| t.as_second());
    let window = |used: f64| Window {
        used: percent(used),
        resets_at,
    };
    let usage = &body["individualUsage"];
    let plan = &usage["plan"];
    let plan_on = plan["enabled"] != false;
    let mut models = Vec::new();
    let mut bucket = |name: &str, used: Option<f64>| {
        if let Some(used) = used {
            models.push(ModelWindow {
                model: name.into(),
                window: window(used),
            });
        }
    };
    if plan_on {
        bucket(CURSOR_MODELS, plan["autoPercentUsed"].as_f64());
        bucket(CURSOR_OTHER_MODELS, plan["apiPercentUsed"].as_f64());
    }
    let on_demand = &usage["onDemand"];
    if on_demand["enabled"] == true {
        bucket(CURSOR_ON_DEMAND, pool_percent(on_demand));
    }
    let plan_window = plan_on
        .then(|| {
            plan["totalPercentUsed"]
                .as_f64()
                .or_else(|| pool_percent(plan))
        })
        .flatten()
        .map(window);
    Limits::Ready {
        session: None,
        weekly: None,
        models,
        plan: plan_window,
    }
}

fn pool_percent(pool: &Value) -> Option<f64> {
    match (pool["used"].as_f64(), pool["limit"].as_f64()) {
        (Some(used), Some(limit)) if limit > 0.0 => Some(used / limit * 100.0),
        _ => pool["totalPercentUsed"].as_f64(),
    }
}

fn percent(used_percent: f64) -> u8 {
    used_percent.round().clamp(0.0, 100.0) as u8
}

fn failed(problem: UsageProblem) -> Limits {
    Limits::Failed { problem }
}

const APP_SERVER: &str = "codex app-server";

fn app_server_failed(e: &std::io::Error) -> UsageProblem {
    run_failed(APP_SERVER, e)
}

fn run_failed(command: &str, e: &std::io::Error) -> UsageProblem {
    if e.kind() == std::io::ErrorKind::NotFound {
        let program = command.split(' ').next().unwrap_or(command);
        return UsageProblem::NotInstalled(program.into());
    }
    UsageProblem::CliFailed {
        command: command.into(),
        detail: e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn old_claude_status_parses() {
        let status: ClaudeStatus = serde_json::from_str(
            r#"{"loggedIn": false, "authMethod": "none", "apiProvider": "firstParty"}"#,
        )
        .unwrap();
        assert!(!status.logged_in);
        assert_eq!(status.config_directory, None);
    }

    #[test]
    fn missing_cli_is_not_installed() {
        let gone = std::io::Error::new(std::io::ErrorKind::NotFound, "program not found");
        assert_eq!(
            app_server_failed(&gone),
            UsageProblem::NotInstalled("codex".into())
        );
        let denied = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        assert!(matches!(
            run_failed("claude", &denied),
            UsageProblem::CliFailed { .. }
        ));
    }

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
    fn cursor_summary_reads_buckets_then_plan() {
        let limits = cursor_summary(&json!({
            "billingCycleStart": "2026-10-20T00:25:01.000Z",
            "billingCycleEnd": "2026-11-20T00:25:01.000Z",
            "membershipType": "enterprise", "limitType": "team", "isUnlimited": false,
            "individualUsage": {
                "plan": {"enabled": true, "used": 2000, "limit": 2000, "remaining": 0,
                         "autoPercentUsed": 0.10095238095238096, "apiPercentUsed": 21.310000000000002,
                         "totalPercentUsed": 3.4944},
                "onDemand": {"enabled": true, "used": 0, "limit": null, "remaining": null}
            },
            "teamUsage": {"onDemand": {"enabled": true, "used": 106523, "limit": null}}
        }));
        let Limits::Ready {
            session,
            weekly,
            models,
            plan,
        } = limits
        else {
            panic!("Ready가 아니다");
        };
        assert!(session.is_none() && weekly.is_none());
        let names: Vec<_> = models
            .iter()
            .map(|m| (m.model.as_str(), m.window.used))
            .collect();
        assert_eq!(names, [("Cursor Models", 0), ("Other Models", 21)]);
        let plan = plan.unwrap();
        assert_eq!(plan.used, 3);
        assert_eq!(plan.resets_at, Some(1795134301));
    }

    #[test]
    fn cursor_summary_hides_disabled_plan_and_shows_capped_on_demand() {
        let Limits::Ready { models, plan, .. } = cursor_summary(&json!({
            "individualUsage": {
                "plan": {"enabled": false, "autoPercentUsed": 0, "apiPercentUsed": 0, "totalPercentUsed": 0},
                "onDemand": {"enabled": true, "used": 500, "limit": 2000}
            }
        })) else {
            panic!("Ready가 아니다");
        };
        assert!(plan.is_none());
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].model, "On-demand");
        assert_eq!(models[0].window.used, 25);
        assert_eq!(models[0].window.resets_at, None);
    }

    #[test]
    fn cursor_token_reads_subject_and_makes_dashboard_cookie() {
        use base64::Engine;
        let body = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"sub":"auth0|user_01ABC","exp":1800000000}"#);
        let raw = format!("h.{body}.s");
        let token = CursorToken::parse(&raw).unwrap();
        assert_eq!(token.subject, "auth0|user_01ABC");
        assert_eq!(token.expires_at, Some(1800000000));
        assert_eq!(
            token.cookie(),
            format!("WorkosCursorSessionToken=auth0%7Cuser_01ABC%3A%3A{raw}")
        );
        assert!(CursorToken::parse("not-a-jwt").is_none());
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
