//! 에이전트 카탈로그와 새 노드의 시작 명령.
//! 카탈로그는 Orca 표를 옮긴 데이터(`agents.json`)다. 바뀌는 이유는 CLI 버전과 Orca 표다.
//! 근거: docs/design.md 「에이전트」.

pub mod claude;
pub mod codex;
pub mod cursor;
pub mod opencode2;

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::LazyLock;

use portable_pty::CommandBuilder;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::graph::{Delivery, Kind, WaitKind};
use crate::screen::{Dialog, Place};
use crate::skills::Launch;

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
    Cursor,
}

/// 훅이 메시지를 꺼내는 때. 훅으로 전달하는 프로필(Claude, Cursor)이 쓴다.
#[derive(Debug, PartialEq)]
pub enum Take {
    /// 턴 안의 지점. 이 방식의 메시지만 바로 꺼낸다.
    Now(&'static [Kind]),
    /// 턴이 없다(세션 시작, 턴 끝). 훅이 메시지가 올 때까지 기다린다.
    WhenIdle,
    /// 메시지를 꺼내지 않는다(알림).
    Never,
    /// 일하는 노드의 도구 호출 뒤. 조정만 꺼내고, 턴 끝까지 기다리는 큐 메시지가 새로 있으면 한 줄로 알린다.
    /// 에이전트가 지금 `loom wait`로 읽을지 고른다. 근거: docs/design.md 「전달 지점」.
    BetweenTools,
}

/// `Take::BetweenTools`에서 꺼내는 방식.
pub const BETWEEN_TOOLS_KINDS: &[Kind] = &[Kind::Steer];

impl Profile {
    /// 계정을 고르는 프로필이면 그 공급자다. OpenCode 2는 공급자가 여럿이라 loom 계정이 없다.
    /// Cursor는 계정 전환을 아직 두지 않는다. 노드는 Cursor CLI의 로그인을 쓴다.
    pub fn provider(self) -> Option<Provider> {
        match self {
            Profile::Claude => Some(Provider::Claude),
            Profile::Codex => Some(Provider::Codex),
            Profile::OpenCode2 | Profile::Cursor => None,
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
            // `<주제> - ⏳ Working ...`, 쉬면 `<주제> - ✅ Ready`다. 주제가 생기기 전에는 `Cursor Agent`다(가설).
            Profile::Cursor => Some(match title.rsplit_once(" - ") {
                Some((rest, state)) if state.split(' ').next().is_some_and(is_spinner) => rest,
                _ => title,
            })
            .filter(|t| *t != "Cursor Agent"),
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
/// 셸·명령 노드와 프로필 없는 에이전트의 확인 창: 마지막 줄의 입력 요청(sudo, ssh, git, y/n).
const SHELL_DIALOGS: [Dialog; 5] = [
    Dialog {
        text: "password",
        kind: WaitKind::Input,
        place: Place::LastLine,
    },
    Dialog {
        text: "passphrase",
        kind: WaitKind::Input,
        place: Place::LastLine,
    },
    Dialog {
        text: "(yes/no",
        kind: WaitKind::Input,
        place: Place::LastLine,
    },
    Dialog {
        text: "[y/n]",
        kind: WaitKind::Input,
        place: Place::LastLine,
    },
    Dialog {
        text: "Username for '",
        kind: WaitKind::Input,
        place: Place::LastLine,
    },
];

/// 노드 터미널 화면에서 찾을 확인 창(`screen::find`). OpenCode 2 세션은 모두 허용으로 띄워 창이 없다.
pub fn dialogs(profile: Option<Profile>) -> &'static [Dialog] {
    match profile {
        Some(Profile::Claude) => &claude::DIALOGS,
        Some(Profile::Codex) => &codex::DIALOGS,
        Some(Profile::Cursor) => &cursor::DIALOGS,
        Some(Profile::OpenCode2) => &[],
        None => &SHELL_DIALOGS,
    }
}

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
        /// 이어 갈 세션(Claude 세션 id, Codex 스레드 id). 계정 전환만 넣는다. 그때 `brief`는 이어 간 세션의 새 입력이다.
        #[serde(skip)]
        #[ts(skip)]
        resume: Option<String>,
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
            LaunchError::UnknownAgent(id) => {
                write!(f, "{}: this agent is not in the catalog.", id.0)
            }
            LaunchError::NotInstalled { agent, command } => write!(
                f,
                "{agent}: the daemon PATH has no `{command}`. Install it, then click Find again in settings."
            ),
            LaunchError::BriefTooLong { len, limit } => write!(
                f,
                "The brief is {len} bytes. Make it {limit} bytes or less, and put long material in a file and send its path."
            ),
            LaunchError::Server { program, cause } => {
                write!(f, "Could not start `{program}`: {cause}")
            }
            LaunchError::NoServer(agent) => {
                write!(f, "{agent}: could not start the server for the node.")
            }
            LaunchError::NoModelChoice(agent) => write!(
                f,
                "{agent}: does not take this `--model` or `--effort` (Claude and Codex take both, Cursor takes `--model` with an optional `--effort`). Fix them and try again."
            ),
            LaunchError::NoBrief(agent) => write!(
                f,
                "{agent}: does not take a brief. Start it without a brief and type into its terminal."
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
/// `plugin`은 loom이 싣는 스킬 플러그인 폴더다. Claude·Cursor만 시작 인자로 받는다. 다른 프로필은 서버가 받는다.
/// `launch`는 상시 지시를 고른다(`skills::always_on`). Claude는 시작 인자로, Cursor는 훅이 읽는 환경 변수로 받는다.
pub fn command(
    start: &Start,
    cwd: &Path,
    remote: Option<Remote<'_>>,
    bypass_off: &[AgentId],
    plugin: &Path,
    launch: Launch,
) -> Result<CommandBuilder, LaunchError> {
    let mut cmd = match start {
        Start::Agent {
            agent,
            brief,
            choice,
            resume,
        } => agent_command(
            agent,
            brief,
            choice,
            resume.as_deref(),
            remote,
            bypass_off,
            plugin,
            launch,
        )?,
        Start::Command { line } if line.trim().is_empty() => default_shell(cwd),
        Start::Command { line } => shell(line),
    };
    cmd.cwd(cwd);
    color_terminal(&mut cmd);
    Ok(cmd)
}

/// 노드·로그인 터미널(xterm.js)은 트루컬러를 그린다. 데몬을 띄운 곳(예: Claude Code의 Bash 도구)이 색을 끈 값은
/// 노드에 맞지 않는다. 남기면 Claude Code 마스코트 같은 색 출력이 흰색으로 나온다.
pub fn color_terminal(cmd: &mut CommandBuilder) {
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env_remove("NO_COLOR");
    cmd.env_remove("FORCE_COLOR");
}

/// 데몬을 띄운 터미널(Claude Code, Codex, OpenCode, Orca, 다른 loom 노드)이 넣은 변수다. 데몬은 시작할 때 지운다.
/// 남기면 노드와 사용량 조회가 데몬을 띄운 곳에 따라 다르게 돈다.
/// - 세션 표지: Claude Code가 자기를 그 세션의 자식으로 보고 대화 기록을 저장하지 않는다
///   ("inherited CLAUDE_CODE_CHILD_SESSION marker", 2.1.304 실측). Orca 훅 변수는 노드의 훅 보고를 Orca로 보낸다.
/// - 로그인 폴더(`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `OPENCODE_CONFIG_DIR`): 기본 계정이 데몬을 띄운 곳의 계정이 된다.
///   다른 `HOME`으로 띄운 시험 데몬도 사람의 `~/.codex`를 썼다. 다른 계정은 loom 계정 설정으로 고른다.
/// - `ANTHROPIC_BASE_URL`: Claude Code는 `settings.json`의 `env`를 자기 셸 명령에 넣는다. 그 세션에서 띄운 데몬이 받으면
///   loom 계정 노드도 시스템 기본 계정의 중계 서버로 요청하고, 그 서버가 다른 토큰을 거절했다(2026-10-21 「401 인증 실패」).
///   시스템 기본 노드는 Claude가 `~/.claude/settings.json`에서 다시 읽으므로 그대로다.
pub fn is_launcher_env(key: &str) -> bool {
    ["CLAUDE_CODE_", "CODEX_", "OPENCODE", "ORCA_", "LOOM_"]
        .iter()
        .any(|prefix| key.starts_with(prefix))
        || matches!(
            key,
            "CLAUDECODE"
                | "CLAUDE_PID"
                | "CLAUDE_EFFORT"
                | "CLAUDE_CONFIG_DIR"
                | "ANTHROPIC_BASE_URL"
        )
}

#[allow(clippy::too_many_arguments)]
fn agent_command(
    id: &AgentId,
    brief: &str,
    choice: &ModelChoice,
    resume: Option<&str>,
    remote: Option<Remote<'_>>,
    bypass_off: &[AgentId],
    plugin: &Path,
    launch: Launch,
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
    cmd.args(leading_args(agent, resume, !bypass_off.contains(id)));
    // 프로필이 있으면 프로필이 시작 인자를 정한다. 카탈로그 `args`는 Orca가 연동 없이 띄울 때의 인자다.
    // 예: OpenCode 2의 `--standalone`은 비공개 서버를 띄워 `--server`와 함께 쓸 수 없다.
    match (agent.profile, remote) {
        (Some(Profile::Claude), _) => cmd.args(claude::args(plugin, launch)),
        (Some(Profile::Cursor), _) => {
            cmd.args(cursor::args(plugin));
            cmd.env(
                cursor::POSITION_ENV,
                cursor::position_value(launch.position),
            );
            cmd.env(cursor::SKILLS_ENV, cursor::skills_value(launch.skills));
        }
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

/// 다른 인자보다 앞에 넣는 인자: 이어 갈 세션(계정 전환)과 모든 권한 허용. Codex `resume`은 하위 명령이라 맨 앞이다.
/// `codex resume --remote`는 권한 인자를 받지 않는다("Permission overrides are not supported when resuming a remote task",
/// 0.159.2 실측). 그 권한은 데몬이 서버에서 스레드를 열 때 넘긴다(`codex::resume`).
fn leading_args(agent: &Agent, resume: Option<&str>, bypass: bool) -> Vec<String> {
    let mut args = match (agent.profile, resume) {
        (Some(Profile::Claude), Some(session)) => vec!["--resume".into(), session.into()],
        (Some(Profile::Codex), Some(thread)) => return vec!["resume".into(), thread.into()],
        _ => Vec::new(),
    };
    if bypass {
        args.extend(agent.bypass.iter().cloned());
    }
    args
}

/// 모든 프로필이 받는 통신 명령. 프로필별 대기 방식은 각 실행 지시가 정한다.
pub const COMMUNICATION_USAGE: &str = "You are a node in a loom graph. Use shell commands to communicate. You can send to any node; talk to your parent and children first, and report to your parent.
- loom spawn <name> --agent <agent-id>: create a child. Supply its brief on stdin or with --file. --worktree <branch> gives it a new worktree.
- loom status: inspect your ID, current parent and children, and their process availability. loom status --all lists every node with its ID, parent and place.
- loom send <name> <body>: queue a message without interrupting a turn. A name finds your parent or child first, then a live node with that unique name. Sending prints the message ID.
- loom send '#<node-id>' <body>: address any node by stable ID. Use IDs if names are ambiguous.
- loom reply <message-id> <body>: reply to a received message. The reply names its request ID.
- loom wait --reply-to <message-id> --timeout <seconds> --json: wait for a particular reply. This also finds replies already delivered by another consumer. Other messages stay in the inbox. A timeout exits with an error.
- loom steer <name-or-id> <body>: insert guidance between tool calls. Steer and interrupt go only to your parent or children.
- loom interrupt <name-or-id> <body>: stop current work and follow this message. If you receive an interrupt, stop your previous task.
- loom done <summary> or loom done --file <path>: report completion to your parent. Completion leaves the process available for feedback. People read the summary on your node card: write it in the language they use with you, not in the agent-to-agent language.
- loom ask <question>: put one question in the human's inbox. When you have no parent, you MUST ask the human anything (a question, a choice, an approval, a permission) only with loom ask, in the one-line form of the ask-human rules below, then end your turn. Never end a turn with a question only in your text. With a parent, ask your parent with loom send, never the human.
- loom answered: when the human's input answers your open loom ask question, you MUST call loom answered first, then act on the answer. If the input does not answer it, leave the question open. If you no longer need the answer, call loom answered.
- loom adopt <name>: become this node's parent. Use loom status after a parent change.
- loom rm <name-or-id>: remove your own child and end its process. Its children become your children; reclaim them next.
Bodies are at most 4KB and completion summaries at most 1KB. Put longer material in a file and send its absolute path.
Messages begin with loom message and have an ID. The same ID can be redelivered; do not repeat completed actions for that ID.";

/// 설치를 보기 전에 거절한다. 받지 않는 인자를 버리고 띄우면 자식이 다른 모델로 돈다.
fn choice_args(agent: &Agent, choice: &ModelChoice) -> Result<Vec<String>, LaunchError> {
    match agent.profile {
        _ if choice.is_empty() => Ok(Vec::new()),
        Some(Profile::Claude) => Ok(claude::choice_args(choice)),
        Some(Profile::Codex) => Ok(codex::choice_args(choice)),
        Some(Profile::Cursor) => cursor::choice_args(choice)
            .ok_or_else(|| LaunchError::NoModelChoice(agent.label.clone())),
        Some(Profile::OpenCode2) | None => Err(LaunchError::NoModelChoice(agent.label.clone())),
    }
}

/// 받는 노드에 넘기는 메시지 본문. 프로필마다 이 글을 맥락이나 새 턴 입력으로 넣는다.
/// 훅이 넘기는 글: 꺼낸 메시지와 새로 기다리는 큐 메시지 알림(`Take::BetweenTools`). 둘 다 없으면 없다.
pub fn hook_text(taken: &[Delivery], queued_from: &[String]) -> Option<String> {
    let mut parts = Vec::new();
    if !taken.is_empty() {
        parts.push(message_text(taken));
    }
    if !queued_from.is_empty() {
        let mut from: Vec<&str> = Vec::new();
        for name in queued_from {
            if !from.contains(&name.as_str()) {
                from.push(name);
            }
        }
        parts.push(format!(
            "loom: {} new queued message(s) waiting (from: {}). Run `loom wait --json` now if they may change your current task.",
            queued_from.len(),
            from.join(", ")
        ));
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

pub fn message_text(taken: &[Delivery]) -> String {
    taken
        .iter()
        .map(|d| {
            let how = match d.kind {
                Kind::Queue => "queue",
                Kind::Steer => "steer",
                Kind::Interrupt => "interrupt. Stop current work and follow this body",
            };
            let reply = d
                .reply_to
                .map(|id| format!(", reply to: #{}", id.0))
                .unwrap_or_default();
            format!(
                "loom message #{} (from: {}, mode: {how}{reply})\n{}",
                d.id.0, d.from, d.body
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(unix)]
fn default_shell(_cwd: &Path) -> CommandBuilder {
    CommandBuilder::new_default_prog()
}

/// `cmd.exe`는 UNC 폴더에서 시작하지 못하고 `C:\Windows`로 간다. WSL 폴더면 그 배포판의 셸을 그 폴더에서 연다.
#[cfg(windows)]
fn default_shell(cwd: &Path) -> CommandBuilder {
    match cwd.to_str().and_then(crate::wsl::location) {
        Some(wsl) => {
            let mut cmd = CommandBuilder::new("wsl.exe");
            cmd.args(["-d", wsl.distro.as_str(), "--cd", wsl.path.as_str()]);
            cmd
        }
        None => CommandBuilder::new_default_prog(),
    }
}

/// 명령 한 줄을 도는 셸. 명령 노드와 설정 스크립트가 쓴다.
#[cfg(unix)]
pub fn shell(line: &str) -> CommandBuilder {
    let mut cmd = CommandBuilder::new("sh");
    cmd.args(["-c", line]);
    cmd
}

#[cfg(windows)]
pub fn shell(line: &str) -> CommandBuilder {
    let mut cmd = CommandBuilder::new("cmd");
    cmd.args(["/C", line]);
    cmd
}

/// 런타임마다 시작 지시에 질문 규칙이 둘 다 든다: 사용법의 필수 한 줄과 `ask-human` 본문 한 번.
/// 자리와 스위치가 무엇이든 든다. `texts`는 한 시작의 (사용법 글, 상시 지시 글)이다.
/// 근거: 2026-10-21 사람 지시 "loom 기능과 직결되므로 스킬이 되었든 다른 방식이 되었든 각 에이전트에 강하게 명시되어야 함".
#[cfg(test)]
pub fn assert_every_launch_asks(texts: impl Fn(Launch) -> (String, String)) {
    use crate::skills::{AlwaysOn, Position};
    let off = AlwaysOn {
        o_mode: false,
        user_doc: false,
        ste100: false,
        raptor3: false,
    };
    for position in [Position::Root, Position::Child] {
        for skills in [AlwaysOn::default(), off] {
            let (usage, rules) = texts(Launch { position, skills });
            assert!(usage.contains("you MUST ask the human anything"), "{usage}");
            let ask = crate::skills::ask_rules();
            assert_eq!(rules.matches(&ask).count(), 1, "{position:?} {skills:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_comes_first_and_codex_resume_takes_no_permission_args() {
        let claude = find(&AgentId("claude".into())).unwrap();
        let codex = find(&AgentId("codex".into())).unwrap();
        assert_eq!(
            leading_args(claude, Some("s1"), true),
            ["--resume", "s1", "--dangerously-skip-permissions"]
        );
        assert_eq!(leading_args(claude, None, false), Vec::<String>::new());
        assert_eq!(leading_args(codex, Some("t1"), true), ["resume", "t1"]);
        assert_eq!(
            leading_args(codex, None, true),
            ["--dangerously-bypass-approvals-and-sandbox"]
        );
    }

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
    fn launcher_env_is_known() {
        for key in [
            "CLAUDE_CODE_CHILD_SESSION",
            "CLAUDECODE",
            "CLAUDE_CONFIG_DIR",
            "CODEX_HOME",
            "OPENCODE_SESSION_ID",
            "OPENCODE",
            "ORCA_AGENT_HOOK_PORT",
            "LOOM_TOKEN",
            "ANTHROPIC_BASE_URL",
        ] {
            assert!(is_launcher_env(key), "{key}");
        }
        for key in [
            "HOME",
            "PATH",
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "CLAUDE",
        ] {
            assert!(!is_launcher_env(key), "{key}");
        }
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
        assert_eq!(ids, ["claude", "codex", "opencode2", "cursor"]);
    }

    /// 실측한 제목(2026-10-20, Claude Code 2.1.304, Codex 0.159.2, OpenCode 2; 2026-10-21, Cursor 2026.10.19).
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
                Profile::Cursor,
                "Sleep Then Done - ⏳ Working ..·",
                Some("Sleep Then Done"),
            ),
            (
                Profile::Cursor,
                "Secret Word Node - ✅ Ready",
                Some("Secret Word Node"),
            ),
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
