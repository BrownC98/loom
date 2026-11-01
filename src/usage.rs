use std::path::Path;
use std::time::Duration;

use api::graph::errors::LaunchError;
use api::settings::usage::{Limits, Usage, UsageProblem};

pub const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

pub fn usage_now(email: Option<String>, plan: Option<String>, limits: Limits) -> Usage {
    Usage {
        email,
        plan,
        limits,
        read_at: jiff::Timestamp::now().as_second(),
    }
}

pub fn usage_failed(problem: UsageProblem) -> Usage {
    usage_now(None, None, Limits::Failed { problem })
}

pub fn launch_failed(e: LaunchError) -> Usage {
    match e {
        LaunchError::NotInstalled { command, .. } => {
            usage_failed(UsageProblem::NotInstalled(command))
        }
        e => usage_failed(UsageProblem::Unavailable(e.to_string())),
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
