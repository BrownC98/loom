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

impl Profile {
    /// CLI가 쓴 터미널 제목에서 지금 하는 일(카드 주제)을 꺼낸다. 아직 주제가 없으면 없다.
    /// 제목 형식은 CLI마다 다르다. 근거: docs/design.md 「그래프와 화면」.
    pub fn topic(self, title: &str) -> Option<String> {
        let title = title.trim();
        let topic = match self {
            // `✳ <주제>`. 주제가 생기기 전에는 `✳ Claude Code`다.
            Profile::Claude => Some(match title.split_once(' ') {
                Some((spinner, rest)) if is_spinner(spinner) => rest,
                _ => title,
            })
            .filter(|t| *t != "Claude Code"),
            // `tui.terminal_title=["thread-title"]`로 띄우면 `<스레드 제목>`, 일하는 동안 `<스레드 제목> ⠋`이다.
            // 제목이 생기기 전에는 스레드 id(UUID)다.
            Profile::Codex => Some(match title.rsplit_once(' ') {
                Some((rest, spinner)) if is_spinner(spinner) => rest,
                _ => title,
            })
            .filter(|t| !is_uuid(t)),
            // `OC | <세션 제목>`. 제목이 생기기 전에는 `OpenCode`다.
            Profile::OpenCode2 => title.strip_prefix("OC | "),
        };
        topic
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
    }
}

/// CLI가 일하는 동안 제목에 붙이는 글자 하나(✳ ◐ ◑ ⠋ 등). 글자·숫자가 아닌 ASCII 밖 기호다.
fn is_spinner(part: &str) -> bool {
    let mut chars = part.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if !c.is_ascii() && !c.is_alphanumeric())
}

/// `01a1573a-9c7e-7b33-a79c-5c196d8346e9` 꼴.
fn is_uuid(text: &str) -> bool {
    text.len() == 36
        && text.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
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

/// 데몬이 다시 시작한 뒤 노드 서버에 다시 붙는 값. 노드와 함께 DB에 남는다(graph).
/// `pid`는 서버 프로세스다. 셸을 찾고 서버를 끝낼 때 쓴다.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ServerLink {
    /// `thread`는 TUI가 만든 노드 스레드다. 만들기 전에는 없다.
    Codex {
        url: String,
        pid: Option<u32>,
        thread: Option<String>,
    },
    OpenCode2 {
        url: String,
        pid: Option<u32>,
        session: String,
        password: String,
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
    /// 승인 창 없이 모든 권한을 허용하는 인자. 비어 있으면 그런 인자를 확인하지 않은 것이다.
    #[serde(default)]
    bypass: Vec<String>,
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
    /// 모든 권한 허용 인자가 있다. 없으면 설정에서 끄고 켤 수 없다.
    pub can_bypass: bool,
    /// 데몬 PATH에서 찾은 실행 파일. 없으면 설치하지 않은 것이다.
    pub path: Option<String>,
    /// 메시지 연동이 그 CLI 버전과 맞지 않을 수 있다는 경고.
    pub version_warning: Option<VersionMismatch>,
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
            can_bypass: !a.bypass.is_empty(),
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
pub fn version_warnings(codex_version: Option<&str>) -> HashMap<Profile, VersionMismatch> {
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
    let out = crate::process_tree::command(cmd)
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
    /// `brief`가 비어 있으면 빈 세션으로 시작한다. 화면은 `choice`를 보내지 않는다.
    Agent {
        agent: AgentId,
        brief: String,
        #[serde(default)]
        #[ts(skip)]
        choice: ModelChoice,
    },
    /// `line`이 비어 있으면 사용자의 기본 셸을 띄운다.
    Command { line: String },
}

/// `loom spawn --model`·`--effort`. 빈 칸은 CLI 설정의 값으로 뜬다.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct ModelChoice {
    pub model: Option<String>,
    pub effort: Option<String>,
}

impl ModelChoice {
    fn is_empty(&self) -> bool {
        self.model.is_none() && self.effort.is_none()
    }
}

impl Start {
    /// 노드 목록에 남기는 에이전트. 명령(기본 셸 포함)이면 없다.
    pub fn agent(&self) -> Option<AgentId> {
        match self {
            Start::Agent { agent, .. } => Some(agent.clone()),
            Start::Command { .. } => None,
        }
    }

    /// 에이전트에게 준 첫 지시. 명령이면 없다(빈 글).
    pub fn brief(&self) -> &str {
        match self {
            Start::Agent { brief, .. } => brief,
            Start::Command { .. } => "",
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

/// 노드를 띄우지 못한 이유. 에이전트에게는 한국어 문장(`Display`)이, 화면에는 코드와 값(JSON)이 간다.
#[derive(Debug, Serialize, TS)]
#[serde(tag = "code", content = "value", rename_all = "camelCase")]
#[ts(export)]
pub enum LaunchError {
    UnknownAgent(AgentId),
    NotInstalled {
        agent: String,
        command: String,
    },
    BriefTooLong {
        #[ts(type = "number")]
        len: usize,
        #[ts(type = "number")]
        limit: usize,
    },
    /// 노드가 붙을 서버(`codex app-server`, `opencode serve`)를 띄우지 못했다. `cause`는 OS·서버 오류 글이다.
    Server {
        program: String,
        cause: String,
    },
    /// 서버를 쓰는 프로필인데 서버가 없다. 값은 에이전트 이름이다.
    NoServer(String),
    /// 첫 지시문을 받지 않는 에이전트다. 값은 에이전트 이름이다.
    NoBrief(String),
    /// 모델·추론 강도를 시작 인자로 받지 않는 에이전트다. 값은 에이전트 이름이다.
    NoModelChoice(String),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LaunchError::UnknownAgent(id) => write!(f, "{}: 카탈로그에 없는 에이전트다.", id.0),
            LaunchError::NotInstalled { agent, command } => write!(
                f,
                "{agent}: 데몬 PATH에서 실행 파일(`{command}`)을 찾지 못했다. 설치한 뒤 설정에서 [다시 찾기]를 누른다."
            ),
            LaunchError::BriefTooLong { len, limit } => write!(
                f,
                "첫 지시문이 {len}바이트다. {limit}바이트 이하로 줄이고, 긴 자료는 파일 경로를 적는다."
            ),
            LaunchError::Server { program, cause } => {
                write!(f, "`{program}`를 띄우지 못했다: {cause}")
            }
            LaunchError::NoServer(agent) => write!(f, "{agent}: 노드가 붙을 서버를 띄우지 못했다."),
            LaunchError::NoModelChoice(agent) => write!(
                f,
                "{agent}: `--model`·`--effort`를 받지 않는다. Claude·Codex만 받는다. 빼고 다시 만든다."
            ),
            LaunchError::NoBrief(agent) => write!(
                f,
                "{agent}: 첫 지시문을 받지 않는다. 지시문을 비우고 만든 뒤 터미널에 입력한다."
            ),
        }
    }
}

/// 설치한 CLI 버전이 메시지 연동을 만든 고정 버전과 다르다. 화면이 프로필마다 경고 문장으로 바꾼다.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct VersionMismatch {
    pub installed: String,
    pub pinned: String,
}

/// `remote`는 노드 TUI가 붙을 서버다. 서버를 쓰는 프로필(Codex, OpenCode 2)에만 쓴다.
/// `bypass_off`에 없는 에이전트는 모든 권한 허용 인자(`bypass`)로 띄운다.
pub fn command(
    start: &Start,
    cwd: &Path,
    remote: Option<Remote<'_>>,
    bypass_off: &[AgentId],
) -> Result<CommandBuilder, LaunchError> {
    let mut cmd = match start {
        Start::Agent {
            agent,
            brief,
            choice,
        } => agent_command(agent, brief, choice, remote, bypass_off)?,
        Start::Command { line } if line.trim().is_empty() => CommandBuilder::new_default_prog(),
        Start::Command { line } => shell(line),
    };
    cmd.cwd(cwd);
    color_terminal(&mut cmd);
    forget_parent_session(&mut cmd);
    Ok(cmd)
}

/// 노드 터미널(xterm.js)은 트루컬러를 그린다. 데몬을 띄운 곳(예: Claude Code의 Bash 도구)이 색을 끈 값은
/// 노드에 맞지 않는다. 남기면 Claude Code 마스코트 같은 색 출력이 흰색으로 나온다.
fn color_terminal(cmd: &mut CommandBuilder) {
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env_remove("NO_COLOR");
    cmd.env_remove("FORCE_COLOR");
}

/// 데몬을 띄운 Claude Code 세션의 표지. 노드가 물려받으면 Claude Code가 자기를 그 세션의 자식으로 보고
/// 대화 기록을 저장하지 않는다("inherited CLAUDE_CODE_CHILD_SESSION marker", 2.1.304 실측).
/// 노드는 독립 세션이다. `CLAUDE_CONFIG_DIR` 같은 설정 변수는 그대로 둔다.
fn is_parent_session_marker(key: &str) -> bool {
    key.starts_with("CLAUDE_CODE_") || matches!(key, "CLAUDECODE" | "CLAUDE_PID" | "CLAUDE_EFFORT")
}

fn forget_parent_session(cmd: &mut CommandBuilder) {
    let markers: Vec<_> = cmd
        .iter_full_env_as_str()
        .map(|(key, _)| key)
        .filter(|key| is_parent_session_marker(key))
        .map(str::to_owned)
        .collect();
    for key in markers {
        cmd.env_remove(key);
    }
}

fn agent_command(
    id: &AgentId,
    brief: &str,
    choice: &ModelChoice,
    remote: Option<Remote<'_>>,
    bypass_off: &[AgentId],
) -> Result<CommandBuilder, LaunchError> {
    let agent = find(id).ok_or_else(|| LaunchError::UnknownAgent(id.clone()))?;
    let choice_args = choice_args(agent, choice)?;
    let program = installed(agent).ok_or_else(|| LaunchError::NotInstalled {
        agent: agent.label.clone(),
        command: agent.cmd.clone(),
    })?;
    if brief.len() > BRIEF_LIMIT_BYTES {
        return Err(LaunchError::BriefTooLong {
            len: brief.len(),
            limit: BRIEF_LIMIT_BYTES,
        });
    }
    let mut cmd = CommandBuilder::from_argv(crate::process_tree::argv(&program));
    // 프로필이 있으면 프로필이 시작 인자를 정한다. 카탈로그 `args`는 Orca가 연동 없이 띄울 때의 인자다.
    // 예: OpenCode 2의 `--standalone`은 비공개 서버를 띄워 `--server`와 함께 쓸 수 없다.
    if !bypass_off.contains(id) {
        cmd.args(&agent.bypass);
    }
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
        (Some(_), _) => return Err(LaunchError::NoServer(agent.label.clone())),
        (None, _) => cmd.args(&agent.args),
    }
    cmd.args(choice_args);
    // 파일 끝 줄바꿈을 지운다. OpenCode 2 TUI는 `--prompt` 값이 줄바꿈으로 끝나면 제출하지 않는다(Windows 실측).
    let brief = brief.trim_end();
    if brief.is_empty() {
        return Ok(cmd);
    }
    match &agent.prompt {
        Prompt::Argv => cmd.arg(brief),
        Prompt::ArgvAfter(sep) | Prompt::Flag(sep) => cmd.args([sep.as_str(), brief]),
        Prompt::None => {
            return Err(LaunchError::NoBrief(agent.label.clone()));
        }
    }
    Ok(cmd)
}

/// 모든 프로필이 받는 통신 명령. 프로필별 대기 방식은 각 실행 지시가 정한다.
pub const COMMUNICATION_USAGE: &str = "You are a node in a loom graph. Use shell commands to communicate with your parent and children only.
- loom spawn <name> --agent <agent-id>: create a child. Supply its brief on stdin or with --file. --worktree <branch> gives it a new worktree.
- loom status: inspect your ID, current parent and children, and their process availability.
- loom send <name> <body>: queue a message without interrupting a turn. Sending prints the message ID.
- loom send '#<node-id>' <body>: address a parent or child by stable ID. Use IDs if names are ambiguous.
- loom reply <message-id> <body>: reply to a received message. The reply names its request ID.
- loom wait --reply-to <message-id> --timeout <seconds> --json: wait for a particular reply. This also finds replies already delivered by another consumer. Other messages stay in the inbox. A timeout exits with an error.
- loom steer <name-or-id> <body>: insert guidance between tool calls.
- loom interrupt <name-or-id> <body>: stop current work and follow this message. If you receive an interrupt, stop your previous task.
- loom done <summary> or loom done --file <path>: report completion to your parent. Completion leaves the process available for feedback.
- loom adopt <name>: become this node's parent. Use loom status after a parent change.
Bodies are at most 4KB and completion summaries at most 1KB. Put longer material in a file and send its absolute path.
Messages begin with loom 메시지 and have an ID. The same ID can be redelivered; do not repeat completed actions for that ID.";

/// 설치를 보기 전에 거절한다. 받지 않는 인자를 버리고 띄우면 자식이 다른 모델로 돈다.
fn choice_args(agent: &Agent, choice: &ModelChoice) -> Result<Vec<String>, LaunchError> {
    match agent.profile {
        _ if choice.is_empty() => Ok(Vec::new()),
        Some(Profile::Claude) => Ok(claude::choice_args(choice)),
        Some(Profile::Codex) => Ok(codex::choice_args(choice)),
        Some(Profile::OpenCode2) | None => Err(LaunchError::NoModelChoice(agent.label.clone())),
    }
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
            let reply = d
                .reply_to
                .map(|id| format!(", 답변 대상: #{}", id.0))
                .unwrap_or_default();
            format!(
                "loom 메시지 #{} (보낸 노드: {}, 방식: {how}{reply})\n{}",
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

    #[test]
    fn parent_session_markers_are_not_inherited() {
        let mut cmd = shell("true");
        cmd.env("CLAUDE_CODE_CHILD_SESSION", "1");
        cmd.env("CLAUDECODE", "1");
        cmd.env("CLAUDE_CONFIG_DIR", "/x");
        forget_parent_session(&mut cmd);
        assert_eq!(cmd.get_env("CLAUDE_CODE_CHILD_SESSION"), None);
        assert_eq!(cmd.get_env("CLAUDECODE"), None);
        assert_eq!(cmd.get_env("CLAUDE_CONFIG_DIR").unwrap(), "/x");
    }

    #[test]
    fn node_terminal_keeps_colors_on() {
        let mut cmd = shell("true");
        cmd.env("NO_COLOR", "1");
        cmd.env("FORCE_COLOR", "0");
        color_terminal(&mut cmd);
        assert_eq!(cmd.get_env("NO_COLOR"), None);
        assert_eq!(cmd.get_env("FORCE_COLOR"), None);
        assert_eq!(cmd.get_env("COLORTERM").unwrap(), "truecolor");
    }

    #[test]
    fn model_choice_goes_to_claude_and_codex_only() {
        let choice = ModelChoice {
            model: Some("claude-sonnet-5-5".into()),
            effort: Some("medium".into()),
        };
        let agent = |id: &str| find(&AgentId(id.into())).unwrap();
        assert_eq!(
            choice_args(agent("claude"), &choice).unwrap(),
            ["--model", "claude-sonnet-5-5", "--effort", "medium"]
        );
        assert_eq!(
            choice_args(agent("codex"), &choice).unwrap(),
            [
                "-m",
                "claude-sonnet-5-5",
                "-c",
                r#"model_reasoning_effort="medium""#
            ]
        );
        for id in ["opencode2", "grok"] {
            assert!(matches!(
                choice_args(agent(id), &choice),
                Err(LaunchError::NoModelChoice(_))
            ));
        }
        assert_eq!(
            choice_args(agent("grok"), &ModelChoice::default()).unwrap(),
            Vec::<String>::new()
        );
    }

    /// 모든 권한 허용 인자는 CLI 도움말로 확인한 셋에만 둔다(docs/design.md 「실측 근거」).
    #[test]
    fn only_checked_agents_can_bypass() {
        let ids: Vec<_> = agents()
            .into_iter()
            .filter(|a| a.can_bypass)
            .map(|a| a.id.0)
            .collect();
        assert_eq!(ids, ["claude", "codex", "opencode2"]);
    }

    /// 실측한 제목(2026-10-20, Claude Code 2.1.304, Codex 0.159.2, OpenCode 2).
    #[test]
    fn topic_comes_from_each_cli_title() {
        let cases = [
            (
                Profile::Claude,
                "✳ A.txt 파일 내용",
                Some("A.txt 파일 내용"),
            ),
            (
                Profile::Claude,
                "◐ a.txt 한 줄 요약",
                Some("a.txt 한 줄 요약"),
            ),
            (
                Profile::Claude,
                "◑ a.txt 한 줄 요약",
                Some("a.txt 한 줄 요약"),
            ),
            (Profile::Claude, "✳ Claude Code", None),
            (Profile::Claude, "Claude Code", None),
            (
                Profile::OpenCode2,
                "OC | a.txt 파일 내용 한 줄 요약",
                Some("a.txt 파일 내용 한 줄 요약"),
            ),
            (Profile::OpenCode2, "OpenCode", None),
            (
                Profile::Codex,
                "읽고 b.rs에 주석 추가해",
                Some("읽고 b.rs에 주석 추가해"),
            ),
            (
                Profile::Codex,
                "요약해 a.txt 내용 ⠋",
                Some("요약해 a.txt 내용"),
            ),
            (Profile::Codex, "01a1573a-9c7e-7b33-a79c-5c196d8346e9", None),
            (
                Profile::Codex,
                "01a1573b-5b38-7911-bc29-11ece871434d ⠋",
                None,
            ),
        ];
        for (profile, title, topic) in cases {
            assert_eq!(
                profile.topic(title).as_deref(),
                topic,
                "{profile:?} {title:?}"
            );
        }
    }
}
