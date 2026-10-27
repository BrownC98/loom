use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Instant;

use serde_json::Value;

use super::{CLI, Codex};
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

fn app_server_failed(e: &std::io::Error) -> UsageProblem {
    run_failed(APP_SERVER, e)
}
