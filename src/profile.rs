//! 에이전트 카탈로그와 새 노드의 시작 명령.
//! 카탈로그는 Orca 표를 옮긴 데이터(`agents.json`)다. 바뀌는 이유는 CLI 버전과 Orca 표다.
//! 근거: docs/design.md 「에이전트」.

pub mod claude;
pub mod codex;
pub mod opencode2;

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
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

/// 한 번에 넘기는 메시지 본문 합의 상한. 첫 메시지는 늘 넘긴다. 남은 메시지는 다음에 넘긴다.
/// Claude는 훅 `additionalContext` 한 개를 10,000자에서 자르고 파일로 돌린다. 모든 프로필이 이 크기로 묶는다.
pub const BATCH_LIMIT_BYTES: usize = 8 * 1024;

/// 메시지 연동(훅·app-server·서버 API)이 있는 에이전트. 근거: docs/design.md 「에이전트 실행」.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Profile {
    Claude,
    Codex,
    #[serde(rename = "opencode2")]
    OpenCode2,
}

impl Profile {
    /// 계정을 고르는 프로필이면 그 공급자다. OpenCode 2는 공급자가 여럿이라 loom 계정이 없다.
    pub fn provider(self) -> Option<Provider> {
        match self {
            Profile::Claude => Some(Provider::Claude),
            Profile::Codex => Some(Provider::Codex),
            Profile::OpenCode2 => None,
        }
    }
}

/// 계정과 사용량이 있는 공급자. 근거: docs/design.md 「계정과 사용량」.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Provider {
    Claude,
    Codex,
}

/// 노드 TUI가 붙을 서버. 데몬이 노드를 띄우기 전에 그 프로필의 서버를 띄운다.
pub enum Remote<'a> {
    Codex {
        url: &'a str,
    },
    OpenCode2 {
        url: &'a str,
        session: &'a str,
        password: &'a str,
    },
}

/// 노드 하나의 메일함. 데몬이 그래프로 만들어 프로필의 전달 작업에 넘긴다. 프로필은 그래프를 모른다.
pub trait Mailbox: Send + Sync + 'static {
    /// 다음 메일 변화나, 확인하지 않은 메시지를 다시 꺼낼 수 있게 될 때 끝나는 future.
    /// 메일함을 보기 전에 만든다. 그래야 그 사이에 온 메시지를 놓치지 않는다.
    fn changed(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
    /// `kinds` 가운데 하나인 메시지를 온 순서로 꺼낸다. 확인(`ack`)하기 전까지는 `delivered`가 아니다.
    /// 첫 메시지는 늘 꺼낸다. 그 뒤 메시지는 본문 합이 `limit_bytes` 이하일 때만 꺼낸다.
    fn take(&self, kinds: &[Kind], limit_bytes: usize) -> Result<Vec<Delivery>, String>;
    /// 꺼낸 메시지를 받는 쪽에 넘겼다고 확인한다. 확인하지 않은 메시지는 나중에 다시 꺼낸다.
    fn ack(&self, taken: &[Delivery]) -> Result<(), String>;
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
    /// 계정을 고르는 공급자. 프로필이 있어도 없을 수 있다(OpenCode 2).
    pub provider: Option<Provider>,
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
            provider: a.profile.and_then(Profile::provider),
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

/// 메시지 연동이 그 CLI 버전과 맞지 않을 수 있는 프로필마다 화면 경고. 데몬이 시작할 때 한 번 부른다.
/// `codex_version`은 데몬이 샌드박스 인자에도 쓰는 `codex --version` 출력이다.
pub fn version_warnings(codex_version: Option<&str>) -> HashMap<Profile, String> {
    let opencode2 = installed_version(opencode2::CMD);
    [
        (
            Profile::Codex,
            codex_version.and_then(codex::version_warning),
        ),
        (
            Profile::OpenCode2,
            opencode2.as_deref().and_then(opencode2::version_warning),
        ),
    ]
    .into_iter()
    .filter_map(|(profile, warning)| Some((profile, warning?)))
    .collect()
}

/// 데몬 PATH의 `<cmd> --version` 출력. 설치하지 않았으면 없다.
fn installed_version(cmd: &str) -> Option<String> {
    let out = std::process::Command::new(cmd)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
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

    /// 메시지 연동이 있는 에이전트면 그 프로필이다.
    pub fn profile(&self) -> Option<Profile> {
        match self {
            Start::Agent { agent, .. } => profile_of(agent),
            Start::Command { .. } => None,
        }
    }
}

/// `remote`는 노드 TUI가 붙을 서버다. 서버를 쓰는 프로필(Codex, OpenCode 2)에만 쓴다.
pub fn command(
    start: &Start,
    cwd: &Path,
    remote: Option<Remote<'_>>,
) -> Result<CommandBuilder, String> {
    let mut cmd = match start {
        Start::Agent { agent, brief } => agent_command(agent, brief, remote)?,
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
    remote: Option<Remote<'_>>,
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
    // 프로필이 있으면 프로필이 시작 인자를 정한다. 카탈로그 `args`는 Orca가 연동 없이 띄울 때의 인자다.
    // 예: OpenCode 2의 `--standalone`은 비공개 서버를 띄워 `--server`와 함께 쓸 수 없다.
    match (agent.profile, remote) {
        (Some(Profile::Claude), _) => cmd.args(claude::args()),
        (Some(Profile::Codex), Some(Remote::Codex { url })) => cmd.args(codex::args(url)),
        (
            Some(Profile::OpenCode2),
            Some(Remote::OpenCode2 {
                url,
                session,
                password,
            }),
        ) => {
            cmd.args(opencode2::args(url, session));
            cmd.env(opencode2::PASSWORD_ENV, password);
        }
        (Some(_), _) => {
            return Err(format!(
                "{}: 노드가 붙을 서버를 띄우지 못했다.",
                agent.label
            ));
        }
        (None, _) => cmd.args(&agent.args),
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
                "loom 메시지 #{} (보낸 노드: {}, 방식: {how})\n{}",
                d.id.0, d.from, d.body
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
        assert_eq!(
            profile_of(&AgentId("opencode2".into())),
            Some(Profile::OpenCode2)
        );
    }
}
