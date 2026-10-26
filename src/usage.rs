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
use crate::protocol::{LaunchError, Provider};

const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const SESSION_WINDOW_MAX_MINS: u64 = 300;
const DEFAULT_RETRY_AFTER_SECS: i64 = 300;
const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const CLAUDE_USER_AGENT: &str = "claude-code/2.1.0";
const CLAUDE_SUBSCRIPTION_LOGIN: &str = "claude.ai";
const CLAUDE_KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
const KEYCHAIN_SERVICE_HASH_LEN: usize = 8;
const SECURITY: &str = "/usr/bin/security";
const CURSOR_ORIGIN: &str = "https://cursor.com";
const CURSOR_USAGE_URL: &str = "https://cursor.com/api/usage-summary";
const CURSOR_AUTH_FILE: &str = ".config/cursor/auth.json";
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

pub fn credentials_fingerprint(
    provider: Provider,
    site: &Site,
    id: Option<&AccountId>,
) -> Option<u64> {
    let file = match provider {
        Provider::Claude => ".credentials.json",
        Provider::Codex => "auth.json",
    };
    fingerprint(&site.login_dir(provider, id)?.join(file))
}

pub fn cursor_credentials_fingerprint(site: &Site) -> Option<u64> {
    fingerprint(&site.home_file(CURSOR_AUTH_FILE)?)
}

fn fingerprint(path: &Path) -> Option<u64> {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let bytes = std::fs::read(path).ok()?;
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    Some(hasher.finish())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeStatus {
    logged_in: bool,
    auth_method: Option<String>,
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
    let service = claude_keychain_service(
        status
            .config_directory
            .as_deref()
            .filter(|_| site.accounts().config_dir(Provider::Claude, id).is_some()),
    );
    let dir = status
        .config_directory
        .map(|dir| site.local(&dir))
        .or_else(|| site.login_dir(Provider::Claude, id));
    let limits = if !status.logged_in {
        Limits::NeedsLogin
    } else if status
        .auth_method
        .is_some_and(|method| method != CLAUDE_SUBSCRIPTION_LOGIN)
    {
        failed(UsageProblem::LoginNotSupported)
    } else if let Some(dir) = dir {
        let file = || {
            std::fs::read(dir.join(".credentials.json"))
                .ok()
                .and_then(|bytes| claude_token(&bytes))
        };
        let token = if cfg!(target_os = "macos") {
            keychain_read(Path::new(SECURITY), &service, PROBE_TIMEOUT)
                .and_then(|bytes| claude_token(&bytes))
                .or_else(file)
        } else {
            file()
        };
        match token {
            Some(token) => claude_limits(&token),
            None => failed(UsageProblem::NoToken("Claude".into())),
        }
    } else {
        failed(UsageProblem::CliFormat("claude auth status".into()))
    };
    Usage::new(status.email, status.subscription_type, limits)
}

fn claude_keychain_service(config_dir: Option<&str>) -> String {
    use sha2::{Digest, Sha256};
    let Some(dir) = config_dir else {
        return CLAUDE_KEYCHAIN_SERVICE.to_owned();
    };
    let hash: String = Sha256::digest(dir.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!(
        "{CLAUDE_KEYCHAIN_SERVICE}-{}",
        &hash[..KEYCHAIN_SERVICE_HASH_LEN]
    )
}

fn keychain_read(security: &Path, service: &str, limit: Duration) -> Option<Vec<u8>> {
    let mut cmd = Command::new(security);
    cmd.args(["find-generic-password", "-s", service, "-w"]);
    crate::timed::output(cmd, limit)
}

fn claude_token(credentials: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(credentials).ok()?["claudeAiOauth"]["accessToken"]
        .as_str()
        .map(str::to_owned)
}

fn claude_limits(token: &str) -> Limits {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(PROBE_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into();
    let response = agent
        .get(CLAUDE_USAGE_URL)
        .header("Authorization", format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", CLAUDE_USER_AGENT)
        .call();
    let mut res = match response {
        Ok(res) => res,
        Err(e) => return failed(UsageProblem::Unavailable(e.to_string())),
    };
    match res.status().as_u16() {
        200..=299 => match res.body_mut().read_json::<Value>() {
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
        401 => failed(UsageProblem::TokenExpired("Claude".into())),
        429 => {
            let header = res
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok());
            failed(UsageProblem::RateLimited {
                retry_at: jiff::Timestamp::now().as_second() + retry_after_secs(header),
            })
        }
        status => failed(UsageProblem::Unavailable(format!("http status: {status}"))),
    }
}

fn retry_after_secs(header: Option<&str>) -> i64 {
    header
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|secs| *secs >= 0)
        .unwrap_or(DEFAULT_RETRY_AFTER_SECS)
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
        return Usage::new(None, None, Limits::NeedsLogin);
    }
    let text = |v: &Value| v.as_str().map(str::to_owned);
    Usage::new(
        text(&account["email"]),
        text(&account["planType"]),
        codex_limits(&limits["rateLimits"]),
    )
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
    let token =
        read(CURSOR_AUTH_FILE).and_then(|v| v["accessToken"].as_str().and_then(CursorToken::parse));
    let limits = match token {
        Some(token) => cursor_limits(&token),
        None if email.is_some() => failed(UsageProblem::NoToken("Cursor".into())),
        None => Limits::NeedsLogin,
    };
    Usage::new(email, None, limits)
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
