//! 에이전트 카탈로그와 새 노드의 시작 명령.
//! 카탈로그는 Orca 표를 옮긴 데이터(`agents.json`)다. 바뀌는 이유는 CLI 버전과 Orca 표다.
//! 근거: docs/design.md 「에이전트」.

pub mod claude;
pub mod codex;

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use portable_pty::CommandBuilder;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::graph::{Delivery, Kind};

/// 첫 지시문 상한. 근거: docs/design.md 「맥락과 크기」.
pub const BRIEF_LIMIT_BYTES: usize = 4 * 1024;

/// 카탈로그의 `id`. 예: `claude`, `gemini`.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentId(pub String);

/// 메시지 연동(훅·app-server)과 계정이 있는 에이전트. 근거: docs/design.md 「프로필」.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Profile {
    Claude,
    Codex,
}

/// 첫 지시문을 넘기는 방법. Orca의 `promptInjectionMode`를 옮겼다.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum Prompt {
    /// 마지막 인자로 넘긴다.
    Argv,
    /// 구분자 뒤 인자로 넘긴다. 예: `-- <지시문>`.
    ArgvAfter(String),
    /// 플래그 값으로 넘긴다. 예: `--prompt <지시문>`.
    Flag(String),
    /// 넘길 방법이 없다. Orca는 시작 뒤 입력창에 써 넣는다. loom은 PTY에 글자를 쓰지 않는다.
    None,
}

#[derive(Deserialize)]
struct Agent {
    id: AgentId,
    label: String,
    cmd: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    args: Vec<String>,
    prompt: Prompt,
    profile: Option<Profile>,
    homepage: String,
}

static CATALOG: LazyLock<Vec<Agent>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("agents.json")).expect("agents.json은 컴파일 때 넣은 표다")
});

/// 설정 화면과 새 노드 창이 보는 카탈로그 한 줄.
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentView {
    pub id: AgentId,
    pub label: String,
    pub homepage: String,
    pub profile: Option<Profile>,
    /// 첫 지시문을 넘길 수 있다.
    pub takes_brief: bool,
    /// 데몬 PATH에서 찾은 실행 파일. 없으면 설치하지 않은 것이다.
    pub path: Option<String>,
    /// 메시지 연동이 그 CLI 버전과 맞지 않을 수 있다는 경고. 예: `app-server 0.160.0 · 고정 0.159.2`.
    pub version_warning: Option<String>,
}

/// 카탈로그 전체를 데몬 PATH에서 찾는다. 부를 때마다 다시 찾는다.
pub fn agents() -> Vec<AgentView> {
    CATALOG
        .iter()
        .map(|a| AgentView {
            id: a.id.clone(),
            label: a.label.clone(),
            homepage: a.homepage.clone(),
            profile: a.profile,
            takes_brief: !matches!(a.prompt, Prompt::None),
            path: installed(a).map(|p| p.to_string_lossy().into_owned()),
            version_warning: None,
        })
        .collect()
}

fn installed(agent: &Agent) -> Option<PathBuf> {
    std::iter::once(&agent.cmd)
        .chain(&agent.aliases)
        .find_map(|cmd| which::which(cmd).ok())
}

fn find(id: &AgentId) -> Option<&'static Agent> {
    CATALOG.iter().find(|a| a.id == *id)
}

/// 메시지 연동이 있는 에이전트면 그 프로필이다.
pub fn profile_of(id: &AgentId) -> Option<Profile> {
    find(id).and_then(|a| a.profile)
}

#[derive(Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Start {
    /// `brief`가 비어 있으면 빈 세션으로 시작한다.
    Agent { agent: AgentId, brief: String },
    /// `line`이 비어 있으면 사용자의 기본 셸을 띄운다.
    Command { line: String },
}

impl Start {
    /// 노드 목록에 남기는 에이전트. 직접 입력이면 없다.
    pub fn agent(&self) -> Option<AgentId> {
        match self {
            Start::Agent { agent, .. } => Some(agent.clone()),
            Start::Command { .. } => None,
        }
    }

    /// 계정을 골라야 하는 에이전트면 그 공급자다.
    pub fn profile(&self) -> Option<Profile> {
        match self {
            Start::Agent { agent, .. } => profile_of(agent),
            Start::Command { .. } => None,
        }
    }
}

/// `app_server`는 Codex 노드가 붙을 app-server 주소다. Codex 에이전트에만 쓴다.
pub fn command(
    start: &Start,
    cwd: &Path,
    app_server: Option<&str>,
) -> Result<CommandBuilder, String> {
    let mut cmd = match start {
        Start::Agent { agent, brief } => agent_command(agent, brief, app_server)?,
        Start::Command { line } if line.trim().is_empty() => CommandBuilder::new_default_prog(),
        Start::Command { line } => shell(line),
    };
    cmd.cwd(cwd);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    Ok(cmd)
}

fn agent_command(
    id: &AgentId,
    brief: &str,
    app_server: Option<&str>,
) -> Result<CommandBuilder, String> {
    let agent = find(id).ok_or_else(|| format!("{}: 카탈로그에 없는 에이전트다.", id.0))?;
    let program = installed(agent).ok_or_else(|| {
        format!(
            "{}: 데몬 PATH에서 실행 파일(`{}`)을 찾지 못했다. 설치한 뒤 설정에서 [다시 찾기]를 누른다.",
            agent.label, agent.cmd
        )
    })?;
    if brief.len() > BRIEF_LIMIT_BYTES {
        return Err(format!(
            "첫 지시문이 {}바이트다. {BRIEF_LIMIT_BYTES}바이트 이하로 줄이고, 긴 자료는 파일 경로를 적는다.",
            brief.len()
        ));
    }
    let mut cmd = CommandBuilder::new(program);
    cmd.args(&agent.args);
    match (agent.profile, app_server) {
        (Some(Profile::Claude), _) => cmd.args(claude::args()),
        (Some(Profile::Codex), Some(url)) => cmd.args(codex::args(url)),
        (Some(Profile::Codex), None) => return Err("Codex app-server를 띄우지 못했다.".into()),
        (None, _) => {}
    }
    if brief.trim().is_empty() {
        return Ok(cmd);
    }
    match &agent.prompt {
        Prompt::Argv => cmd.arg(brief),
        Prompt::ArgvAfter(sep) | Prompt::Flag(sep) => cmd.args([sep.as_str(), brief]),
        Prompt::None => {
            return Err(format!(
                "{}: 첫 지시문을 받지 않는다. 지시문을 비우고 만든 뒤 터미널에 입력한다.",
                agent.label
            ));
        }
    }
    Ok(cmd)
}

/// 받는 노드에 넘기는 메시지 본문. 프로필마다 이 글을 맥락이나 새 턴 입력으로 넣는다.
pub fn message_text(taken: &[Delivery]) -> String {
    taken
        .iter()
        .map(|d| {
            let how = match d.kind {
                Kind::Queue => "큐",
                Kind::Steer => "조정",
                Kind::Interrupt => "중단. 하던 일을 멈추고 이 본문을 따른다",
            };
            format!(
                "loom 메시지 (보낸 노드: {}, 방식: {how})\n{}",
                d.from, d.body
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_parses_and_ids_are_unique() {
        let mut ids: Vec<_> = CATALOG.iter().map(|a| &a.id.0).collect();
        let len = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), len);
        assert!(
            find(&AgentId("claude".into())).is_some_and(|a| a.profile == Some(Profile::Claude))
        );
    }
}
