use std::path::PathBuf;
use std::process::Command;

use serde_json::{Value, json};

use super::{CLI, Codex, stdio};
use crate::accounts::{AccountId, Site};
use crate::profile::adapter::Meter;
use crate::usage::{
    self, Limits, PROBE_TIMEOUT, Usage, UsageProblem, Window, failed, percent, run_failed,
};

const SESSION_WINDOW_MAX_MINS: u64 = 300;
const APP_SERVER: &str = "codex app-server";
const CREDENTIALS: &str = "auth.json";

impl Meter for Codex {
    fn credentials(&self, site: &Site, id: Option<&AccountId>) -> Option<PathBuf> {
        Some(site.login_dir(&CLI, id)?.join(CREDENTIALS))
    }

    fn read(&self, site: &Site, id: Option<&AccountId>) -> Usage {
        match site.command(&CLI, &["app-server"], id) {
            Ok(cmd) => account(cmd),
            Err(e) => usage::launch_failed(e),
        }
    }
}

fn account(cmd: Command) -> Usage {
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
        rate_limits(&limits["rateLimits"]),
    )
}

fn rate_limits(rate_limits: &Value) -> Limits {
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

fn app_server(cmd: Command) -> Result<[Value; 2], UsageProblem> {
    let calls = [
        ("account/read", json!({ "refreshToken": false })),
        ("account/rateLimits/read", Value::Null),
    ];
    stdio::replies(cmd, calls, PROBE_TIMEOUT).map_err(|failure| match failure {
        stdio::Failure::Run(e) => run_failed(APP_SERVER, &e),
        stdio::Failure::NoReply => UsageProblem::NoReply(APP_SERVER.into()),
    })
}
