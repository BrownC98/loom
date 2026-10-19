//! 프로필: 새 노드를 어떤 명령으로 띄울지 정한다.
//! 바뀌는 이유는 CLI 버전이다. 훅·app-server 연결은 3·4단위에서 여기에 더한다.

use std::path::Path;

use portable_pty::CommandBuilder;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 첫 지시문 상한. 근거: docs/design.md 「맥락과 크기」.
pub const BRIEF_LIMIT_BYTES: usize = 4 * 1024;

#[derive(Clone, Copy, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Profile {
    Claude,
    Codex,
    Command,
}

#[derive(Deserialize, TS)]
#[serde(tag = "profile", rename_all = "camelCase")]
#[ts(export)]
pub enum Start {
    /// `brief`가 비어 있으면 빈 세션으로 시작한다.
    Claude {
        brief: String,
    },
    Codex {
        brief: String,
    },
    /// `line`이 비어 있으면 사용자의 기본 셸을 띄운다.
    Command {
        line: String,
    },
}

impl Start {
    pub fn profile(&self) -> Profile {
        match self {
            Start::Claude { .. } => Profile::Claude,
            Start::Codex { .. } => Profile::Codex,
            Start::Command { .. } => Profile::Command,
        }
    }
}

pub fn command(start: &Start, cwd: &Path) -> Result<CommandBuilder, String> {
    let mut cmd = match start {
        Start::Claude { brief } => agent("claude", brief)?,
        Start::Codex { brief } => agent("codex", brief)?,
        Start::Command { line } if line.trim().is_empty() => CommandBuilder::new_default_prog(),
        Start::Command { line } => shell(line),
    };
    cmd.cwd(cwd);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    Ok(cmd)
}

fn agent(program: &str, brief: &str) -> Result<CommandBuilder, String> {
    if brief.len() > BRIEF_LIMIT_BYTES {
        return Err(format!(
            "첫 지시문이 {}바이트다. {BRIEF_LIMIT_BYTES}바이트 이하로 줄이고, 긴 자료는 파일 경로를 적는다.",
            brief.len()
        ));
    }
    let mut cmd = CommandBuilder::new(program);
    if !brief.trim().is_empty() {
        cmd.arg(brief);
    }
    Ok(cmd)
}

#[cfg(unix)]
fn shell(line: &str) -> CommandBuilder {
    let mut cmd = CommandBuilder::new("sh");
    cmd.args(["-c", line]);
    cmd
}

#[cfg(windows)]
fn shell(line: &str) -> CommandBuilder {
    let mut cmd = CommandBuilder::new("cmd");
    cmd.args(["/C", line]);
    cmd
}
