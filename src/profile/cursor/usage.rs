use std::path::PathBuf;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::Value;

use super::Cursor;
use crate::accounts::{AccountId, Site};
use crate::profile::adapter::Meter;
use crate::usage::{
    Limits, ModelWindow, PROBE_TIMEOUT, Usage, UsageProblem, Window, failed, percent,
};

const ORIGIN: &str = "https://cursor.com";
const USAGE_URL: &str = "https://cursor.com/api/usage-summary";
const AUTH_FILE: &str = ".config/cursor/auth.json";
const COMMAND: &str = "cursor-agent";
const COOKIE_KEEPS: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');
const MODELS: &str = "Cursor Models";
const OTHER_MODELS: &str = "Other Models";
const ON_DEMAND: &str = "On-demand";

impl Meter for Cursor {
    fn credentials(&self, site: &Site, _: Option<&AccountId>) -> Option<PathBuf> {
        site.home_file(AUTH_FILE)
    }

    fn read(&self, site: &Site, _: Option<&AccountId>) -> Usage {
        if !site.installed(COMMAND) {
            return Usage::failed(UsageProblem::NotInstalled(COMMAND.into()));
        }
        let read = |rel: &str| {
            site.home_file(rel)
                .and_then(|path| std::fs::read(path).ok())
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        };
        let email = read(".cursor/cli-config.json")
            .and_then(|v| v["authInfo"]["email"].as_str().map(str::to_owned));
        let token = read(AUTH_FILE).and_then(|v| v["accessToken"].as_str().and_then(Token::parse));
        let limits = match token {
            Some(token) => limits(&token),
            None if email.is_some() => failed(UsageProblem::NoToken("Cursor".into())),
            None => Limits::NeedsLogin,
        };
        Usage::new(email, None, limits)
    }
}

struct Token {
    raw: String,
    subject: String,
    expires_at: Option<i64>,
}

impl Token {
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

fn limits(token: &Token) -> Limits {
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
        .get(USAGE_URL)
        .header("Cookie", token.cookie())
        .header("Accept", "application/json")
        .header("Origin", ORIGIN)
        .header("Referer", format!("{ORIGIN}/dashboard"))
        .call();
    match response {
        Ok(mut res) => match res.body_mut().read_json::<Value>() {
            Ok(body) => summary(&body),
            Err(_) => failed(UsageProblem::ResponseFormat),
        },
        Err(ureq::Error::StatusCode(401)) => expired(),
        Err(e) => failed(UsageProblem::Unavailable(e.to_string())),
    }
}

fn summary(body: &Value) -> Limits {
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
        bucket(MODELS, plan["autoPercentUsed"].as_f64());
        bucket(OTHER_MODELS, plan["apiPercentUsed"].as_f64());
    }
    let on_demand = &usage["onDemand"];
    if on_demand["enabled"] == true {
        bucket(ON_DEMAND, pool_percent(on_demand));
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
