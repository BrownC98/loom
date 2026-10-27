use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use super::Claude;
use super::accounts::CLI;
use crate::accounts::{AccountId, Site};
use crate::profile::adapter::Meter;
use crate::usage::{
    self, Limits, ModelWindow, PROBE_TIMEOUT, Usage, UsageProblem, Window, failed, percent,
    run_failed,
};

const DEFAULT_RETRY_AFTER_SECS: i64 = 300;
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const USER_AGENT: &str = "claude-code/2.1.0";
const SUBSCRIPTION_LOGIN: &str = "claude.ai";
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
const KEYCHAIN_SERVICE_HASH_LEN: usize = 8;
const SECURITY: &str = "/usr/bin/security";
const CREDENTIALS: &str = ".credentials.json";

impl Meter for Claude {
    fn credentials(&self, site: &Site, id: Option<&AccountId>) -> Option<PathBuf> {
        Some(site.login_dir(&CLI, id)?.join(CREDENTIALS))
    }

    fn read(&self, site: &Site, id: Option<&AccountId>) -> Usage {
        match site.command(&CLI, &["auth", "status", "--json"], id) {
            Ok(cmd) => status(cmd, site, id),
            Err(e) => usage::launch_failed(e),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    logged_in: bool,
    auth_method: Option<String>,
    email: Option<String>,
    subscription_type: Option<String>,
    config_directory: Option<String>,
}

fn status(mut cmd: Command, site: &Site, id: Option<&AccountId>) -> Usage {
    let status: Status = match cmd.stdin(Stdio::null()).stderr(Stdio::null()).output() {
        Ok(out) => match serde_json::from_slice(&out.stdout) {
            Ok(status) => status,
            Err(_) => {
                return Usage::failed(UsageProblem::CliFormat("claude auth status".into()));
            }
        },
        Err(e) => return Usage::failed(run_failed("claude", &e)),
    };
    let service = keychain_service(
        status
            .config_directory
            .as_deref()
            .filter(|_| site.accounts().config_dir(&CLI, id).is_some()),
    );
    let dir = status
        .config_directory
        .map(|dir| site.local(&dir))
        .or_else(|| site.login_dir(&CLI, id));
    let limits = if !status.logged_in {
        Limits::NeedsLogin
    } else if status
        .auth_method
        .is_some_and(|method| method != SUBSCRIPTION_LOGIN)
    {
        failed(UsageProblem::LoginNotSupported)
    } else if let Some(dir) = dir {
        let file = || {
            std::fs::read(dir.join(CREDENTIALS))
                .ok()
                .and_then(|bytes| oauth_token(&bytes))
        };
        let token = if cfg!(target_os = "macos") {
            keychain_read(Path::new(SECURITY), &service, PROBE_TIMEOUT)
                .and_then(|bytes| oauth_token(&bytes))
                .or_else(file)
        } else {
            file()
        };
        match token {
            Some(token) => oauth_limits(&token),
            None => failed(UsageProblem::NoToken("Claude".into())),
        }
    } else {
        failed(UsageProblem::CliFormat("claude auth status".into()))
    };
    Usage::new(status.email, status.subscription_type, limits)
}

fn keychain_service(config_dir: Option<&str>) -> String {
    use sha2::{Digest, Sha256};
    let Some(dir) = config_dir else {
        return KEYCHAIN_SERVICE.to_owned();
    };
    let hash: String = Sha256::digest(dir.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("{KEYCHAIN_SERVICE}-{}", &hash[..KEYCHAIN_SERVICE_HASH_LEN])
}

fn keychain_read(security: &Path, service: &str, limit: Duration) -> Option<Vec<u8>> {
    let mut cmd = Command::new(security);
    cmd.args(["find-generic-password", "-s", service, "-w"]);
    crate::timed::output(cmd, limit)
}

fn oauth_token(credentials: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(credentials).ok()?["claudeAiOauth"]["accessToken"]
        .as_str()
        .map(str::to_owned)
}

fn oauth_limits(token: &str) -> Limits {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(PROBE_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into();
    let response = agent
        .get(USAGE_URL)
        .header("Authorization", format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", USER_AGENT)
        .call();
    let mut res = match response {
        Ok(res) => res,
        Err(e) => return failed(UsageProblem::Unavailable(e.to_string())),
    };
    match res.status().as_u16() {
        200..=299 => match res.body_mut().read_json::<Value>() {
            Ok(body) => Limits::Ready {
                session: window(
                    &body["five_hour"]["utilization"],
                    &body["five_hour"]["resets_at"],
                ),
                weekly: window(
                    &body["seven_day"]["utilization"],
                    &body["seven_day"]["resets_at"],
                ),
                models: models(&body["limits"]),
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

fn window(used: &Value, resets_at: &Value) -> Option<Window> {
    Some(Window {
        used: percent(used.as_f64()?),
        resets_at: resets_at
            .as_str()
            .and_then(|s| s.parse::<jiff::Timestamp>().ok())
            .map(|t| t.as_second()),
    })
}

fn models(limits: &Value) -> Vec<ModelWindow> {
    let Some(items) = limits.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter(|v| v["kind"] == "weekly_scoped")
        .filter_map(|v| {
            Some(ModelWindow {
                model: v["scope"]["model"]["display_name"].as_str()?.to_owned(),
                window: window(&v["percent"], &v["resets_at"])?,
            })
        })
        .collect()
}
