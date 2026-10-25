pub mod claude;
pub mod codex;
pub mod cursor;
pub mod omp;
pub mod opencode2;
pub mod pi;

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::LazyLock;

use portable_pty::CommandBuilder;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::protocol::{
    AgentId, BRIEF_LIMIT_BYTES, Delivery, InputField, Kind, LaunchError, ModelChoice, OpenQuestion,
    Prompt as ScreenPrompt, Provider, StartDigests, WaitKind,
};
use crate::screen::{self, Dialog, Grammar, LinePrompt, Reply, Screen, Session};
use crate::skills::Launch;
use crate::wsl::Inside;

pub const BATCH_LIMIT_BYTES: usize = 8 * 1024;
pub const RESUMED_PROMPT: &str = "loom relaunched your process. If your last turn was cut, continue it. Otherwise end your turn.";

/// 메시지 연동(훅·app-server·서버 API)이 있는 에이전트. 근거: docs/design/에이전트-실행.md.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Profile {
    Claude,
    Codex,
    #[serde(rename = "opencode2")]
    OpenCode2,
    Cursor,
    Pi,
    Omp,
}

#[derive(Debug, PartialEq)]
pub enum Take {
    Now(&'static [Kind]),
    WhenIdle,
    Never,
}

impl Profile {
    pub fn provider(self) -> Option<Provider> {
        match self {
            Profile::Claude => Some(Provider::Claude),
            Profile::Codex => Some(Provider::Codex),
            Profile::OpenCode2 | Profile::Cursor | Profile::Pi | Profile::Omp => None,
        }
    }
}

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

pub trait Mailbox: Send + Sync + 'static {
    fn changed(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
    fn take(&self, kinds: &[Kind], limit_bytes: usize) -> Result<Vec<Delivery>, String>;
    fn ack(&self, taken: &[Delivery]) -> Result<(), String>;
    fn release(&self, taken: &[Delivery]) -> Result<(), String>;
    fn turn_ended(&self, reply: Option<String>) -> Option<String>;
    fn human_input(&self) -> Option<String>;
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum Prompt {
    Argv,
    ArgvAfter(String),
    Flag(String),
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
    /// 모든 권한 허용 인자가 있다. 없으면 설정에서 끄고 켤 수 없다. 위치와 관계없이 하나다.
    pub can_bypass: bool,
    /// 위치마다 찾은 결과. 첫째가 이 PC, 그 뒤가 WSL 배포판이다. 모든 에이전트가 같은 위치를 같은 순서로 갖는다.
    pub places: Vec<AgentAt>,
}

/// 한 위치(이 PC 또는 WSL 배포판)에서 에이전트를 찾은 결과.
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentAt {
    /// WSL 배포판 이름. 이 PC면 없다.
    pub distro: Option<String>,
    /// 그 위치의 `PATH`에서 찾은 실행 파일. 없으면 그 위치에 설치하지 않은 것이다.
    pub path: Option<String>,
    /// 메시지 연동이 그 CLI 버전과 맞지 않을 수 있다는 경고.
    pub version_warning: Option<VersionMismatch>,
}

pub fn agents(wsl: &[&Inside]) -> Vec<AgentView> {
    CATALOG
        .iter()
        .map(|a| {
            let names = || std::iter::once(&a.cmd).chain(&a.aliases);
            let pc = AgentAt {
                distro: None,
                path: installed(a).map(|p| p.to_string_lossy().into_owned()),
                version_warning: None,
            };
            let inside = wsl.iter().map(|inside| AgentAt {
                distro: Some(inside.distro.clone()),
                path: inside.find(names()).map(str::to_owned),
                version_warning: None,
            });
            AgentView {
                id: a.id.clone(),
                label: a.label.clone(),
                homepage: a.homepage.clone(),
                profile: a.profile,
                provider: a.profile.and_then(Profile::provider),
                can_bypass: !a.bypass.is_empty(),
                places: std::iter::once(pc).chain(inside).collect(),
            }
        })
        .collect()
}

pub fn catalog_commands() -> Vec<&'static str> {
    CATALOG
        .iter()
        .flat_map(|a| std::iter::once(&a.cmd).chain(&a.aliases))
        .map(String::as_str)
        .collect()
}

fn installed(agent: &Agent) -> Option<PathBuf> {
    std::iter::once(&agent.cmd)
        .chain(&agent.aliases)
        .find_map(|cmd| crate::search_path::find(cmd))
}

pub fn commands(start: &Start) -> Vec<&'static str> {
    match start {
        Start::Agent { agent, .. } => find(agent)
            .map(|a| {
                std::iter::once(&a.cmd)
                    .chain(&a.aliases)
                    .map(String::as_str)
                    .collect()
            })
            .unwrap_or_default(),
        Start::Command { .. } => Vec::new(),
    }
}

fn find(id: &AgentId) -> Option<&'static Agent> {
    CATALOG.iter().find(|a| a.id == *id)
}

pub(crate) fn check_known_choice(id: &AgentId, choice: &ModelChoice) -> Result<(), LaunchError> {
    let agent = find(id).ok_or_else(|| LaunchError::UnknownAgent(id.clone()))?;
    choice_args(agent, choice).map(drop)
}

pub(crate) fn check_choice(id: &AgentId, choice: &ModelChoice) -> Result<(), LaunchError> {
    find(id).map_or(Ok(()), |agent| choice_args(agent, choice).map(drop))
}

pub const PINNED: [(Profile, &str); 2] = [
    (Profile::Codex, "codex"),
    (Profile::OpenCode2, opencode2::CMD),
];

pub fn version_warning(profile: Profile, version_output: &str) -> Option<VersionMismatch> {
    match profile {
        Profile::Codex => codex::version_warning(version_output),
        Profile::OpenCode2 => opencode2::version_warning(version_output),
        _ => None,
    }
}

pub fn write_extensions(dir: &Path) -> std::io::Result<()> {
    pi::write_extensions(dir)?;
    omp::write_extension(dir)
}

pub fn version_warnings(codex_version: Option<&str>) -> HashMap<Profile, VersionMismatch> {
    PINNED
        .into_iter()
        .filter_map(|(profile, cmd)| {
            let output = match profile {
                Profile::Codex => codex_version.map(str::to_owned),
                _ => installed_version(cmd),
            }?;
            Some((profile, version_warning(profile, &output)?))
        })
        .collect()
}

fn installed_version(cmd: &str) -> Option<String> {
    let out = crate::process_tree::command(cmd)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

const SHELL: [LinePrompt; 5] = [
    LinePrompt {
        text: "password",
        reply: Reply::Field(InputField::Secret),
    },
    LinePrompt {
        text: "passphrase",
        reply: Reply::Field(InputField::Secret),
    },
    LinePrompt {
        text: "(yes/no",
        reply: Reply::Words(&[("yes", "yes"), ("no", "no")]),
    },
    LinePrompt {
        text: "[y/n]",
        reply: Reply::Words(&[("y", "yes"), ("n", "no")]),
    },
    LinePrompt {
        text: "Username for '",
        reply: Reply::Field(InputField::Text),
    },
];

pub fn reads_screen(profile: Option<Profile>) -> bool {
    profile != Some(Profile::OpenCode2)
}

pub fn wait(
    profile: Option<Profile>,
    screen: &Screen,
    session: Session,
) -> Option<(WaitKind, String)> {
    match agent_screen(profile) {
        Some((_, dialogs)) => screen::find(dialogs, &screen.text, session),
        None => screen::line(&SHELL, &screen.to_cursor).map(|(text, _)| (WaitKind::Input, text)),
    }
}

pub fn prompt(profile: Option<Profile>, screen: &Screen) -> Option<ScreenPrompt> {
    match agent_screen(profile) {
        Some((grammars, dialogs)) => {
            screen::prompt(grammars, dialogs, &screen.text, screen.cols.into())
        }
        None => screen::line(&SHELL, &screen.to_cursor).map(|(_, prompt)| prompt),
    }
}

fn agent_screen(profile: Option<Profile>) -> Option<(&'static [Grammar], &'static [Dialog])> {
    match profile? {
        Profile::Claude => Some((&[claude::PROMPT], &claude::DIALOGS)),
        Profile::Codex => Some((&[codex::PROMPT], &codex::DIALOGS)),
        Profile::Cursor => Some((&[cursor::PROMPT], &cursor::DIALOGS)),
        Profile::OpenCode2 | Profile::Pi | Profile::Omp => Some((&[], &[])),
    }
}

pub fn profile_of(id: &AgentId) -> Option<Profile> {
    find(id).and_then(|a| a.profile)
}

pub fn linked(id: &AgentId) -> bool {
    profile_of(id).is_some()
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
        /// 이어 갈 세션(Claude·Cursor 세션 id, Codex 스레드 id, OpenCode 2 세션 id, Pi·OMP 세션 파일). 다시 띄우기만 넣는다. 그때 `brief`는 이어 간 세션의 새 입력이다.
        #[serde(skip)]
        #[ts(skip)]
        resume: Option<String>,
    },
    /// `line`이 비어 있으면 사용자의 기본 셸을 띄운다.
    Command { line: String },
}

impl Start {
    pub fn agent(&self) -> Option<AgentId> {
        match self {
            Start::Agent { agent, .. } => Some(agent.clone()),
            Start::Command { .. } => None,
        }
    }

    pub fn profile(&self) -> Option<Profile> {
        match self {
            Start::Agent { agent, .. } => profile_of(agent),
            Start::Command { .. } => None,
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

pub fn command(
    start: &Start,
    cwd: &Path,
    inside: Option<&Inside>,
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
            inside,
            remote,
            bypass_off,
            plugin,
            launch,
        )?,
        Start::Command { line } => match inside {
            Some(_) => CommandBuilder::from_argv(crate::wsl::shell(line)),
            None if line.trim().is_empty() => CommandBuilder::new_default_prog(),
            None => shell(line),
        },
    };
    cmd.cwd(cwd);
    crate::process_tree::color_terminal(&mut cmd);
    Ok(cmd)
}

pub fn is_launcher_env(key: &str) -> bool {
    (["CLAUDE_CODE_", "CODEX_", "OPENCODE", "ORCA_", "LOOM_"]
        .iter()
        .any(|prefix| key.starts_with(prefix))
        && key != crate::open::OPENER_ENV)
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
    inside: Option<&Inside>,
    remote: Option<Remote<'_>>,
    bypass_off: &[AgentId],
    plugin: &Path,
    launch: Launch,
) -> Result<CommandBuilder, LaunchError> {
    let agent = find(id).ok_or_else(|| LaunchError::UnknownAgent(id.clone()))?;
    let choice_args = choice_args(agent, choice)?;
    let not_installed = || LaunchError::NotInstalled {
        agent: agent.label.clone(),
        command: agent.cmd.clone(),
        distro: inside.map(|i| i.distro.clone()),
    };
    let argv = match inside {
        Some(inside) => vec![
            inside
                .find(std::iter::once(&agent.cmd).chain(&agent.aliases))
                .ok_or_else(not_installed)?
                .into(),
        ],
        None => crate::process_tree::argv(&installed(agent).ok_or_else(not_installed)?),
    };
    if brief.len() > BRIEF_LIMIT_BYTES {
        return Err(LaunchError::BriefTooLong {
            len: brief.len(),
            limit: BRIEF_LIMIT_BYTES,
        });
    }
    let mut cmd = CommandBuilder::from_argv(argv);
    cmd.args(leading_args(agent, resume, !bypass_off.contains(id)));
    match (agent.profile, remote) {
        (Some(Profile::Claude), _) => cmd.args(claude::args(plugin, launch, brief)),
        (Some(Profile::Cursor), _) => {
            cmd.args(cursor::args(plugin));
            cmd.env(
                cursor::POSITION_ENV,
                cursor::position_value(launch.position),
            );
            cmd.env(cursor::SKILLS_ENV, cursor::skills_value(launch.skills));
        }
        (Some(Profile::Pi), _) => cmd.args(pi::args(plugin, launch, brief)),
        (Some(Profile::Omp), _) => cmd.args(omp::args(plugin, launch, brief)),
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

fn leading_args(agent: &Agent, resume: Option<&str>, bypass: bool) -> Vec<String> {
    let mut args = match (agent.profile, resume) {
        (Some(Profile::Claude | Profile::Cursor), Some(session)) => {
            vec!["--resume".into(), session.into()]
        }
        (Some(Profile::Codex), Some(thread)) => return vec!["resume".into(), thread.into()],
        (Some(Profile::Pi), Some(session)) => pi::resume_args(session),
        (Some(Profile::Omp), Some(session)) => omp::resume_args(session),
        _ => Vec::new(),
    };
    if bypass {
        args.extend(agent.bypass.iter().cloned());
    }
    args
}

pub const COMMUNICATION_USAGE: &str = concat!("You are a node in a loom graph. Use these shell commands. A target is a node name, a node ID (12), or parent.
- loom spawn <name> --role <role> [--worktree <branch>]: create a child. The role's settings choose its agent, model and effort (loom roles lists them). Give its brief on stdin or with --file <path>, with the heads Goal, Facts, Scope, Done when, Checks, Time. The child gets its task folder in $LOOM_TASK_DIR (brief.md, result.md, review/, logs/): --worktree makes a new task, else the child shares yours. Pick the role from loom roles by its duty. With no parent, each child needs --worktree or is a reviewer of your own branch.
- loom status: your ID, parent and children, and whether their processes are alive. loom status --all lists every node.
", crate::message_usage!(), "- loom answered [Q12 ...]: when the human's input answers your open questions, call this first, then act on the answer.
- loom adopt <name>: become that node's parent. loom rm <target>: remove your child and end its process; its children become yours.
- loom relaunch [<child>]: restart your own (or your child's) agent process with the current loom settings; the conversation continues after the turn ends. Add --agent <agent-id> to give the node to a new session of that agent; the node keeps its role, mailbox, and children.
Run a command in the background if you do not need its result for your next step (for example: a build, a test, a deploy wait, a server). How long it takes is not the rule. Run a short command in the foreground if you need its result now. Messages reach you only between tool calls.
With a parent, write no progress in the terminal. Results go in loom done or loom note. End each turn with at most one short line.
Bodies are at most 4KB and summaries at most 1KB. Put longer material in a file and send its absolute path.
Messages begin with \"loom message #<id> (from: <sender>, kind: <kind>)\", then one line per field. Kinds from loom (exited, setup-failed, removed, adopted, human-input) are notices. The same ID can come again; do not repeat completed actions for that ID.");

pub fn start_digests(profile: Profile, launch: Launch) -> StartDigests {
    let mut digests = crate::skills::part_digests(launch);
    digests.insert(usage_part(profile).to_owned(), usage_digest(profile));
    digests
}

pub fn changed_parts(got: &StartDigests) -> Vec<&str> {
    got.iter()
        .filter(|(part, digest)| current_digest(part).as_ref() != Some(*digest))
        .map(|(part, _)| part.as_str())
        .collect()
}

fn current_digest(part: &str) -> Option<String> {
    let profiles = [
        Profile::Claude,
        Profile::Codex,
        Profile::OpenCode2,
        Profile::Cursor,
        Profile::Pi,
        Profile::Omp,
    ];
    match profiles.into_iter().find(|p| usage_part(*p) == part) {
        Some(profile) => Some(usage_digest(profile)),
        None => crate::skills::part_digest(part),
    }
}

fn usage_part(profile: Profile) -> &'static str {
    match profile {
        Profile::Claude => "usage/claude",
        Profile::Codex => "usage/codex",
        Profile::OpenCode2 => "usage/opencode2",
        Profile::Cursor => "usage/cursor",
        Profile::Pi => "usage/pi",
        Profile::Omp => "usage/omp",
    }
}

fn usage_digest(profile: Profile) -> String {
    let waiting = match profile {
        Profile::Claude => claude::WAITING,
        Profile::Codex => codex::WAITING,
        Profile::OpenCode2 => opencode2::WAITING,
        Profile::Cursor => cursor::WAITING,
        Profile::Pi => pi::WAITING,
        Profile::Omp => omp::WAITING,
    };
    crate::skills::digest(&[COMMUNICATION_USAGE, waiting])
}

fn choice_args(agent: &Agent, choice: &ModelChoice) -> Result<Vec<String>, LaunchError> {
    match agent.profile {
        _ if choice.is_empty() => Ok(Vec::new()),
        Some(Profile::Claude) => Ok(claude::choice_args(choice)),
        Some(Profile::Codex) => Ok(codex::choice_args(choice)),
        Some(Profile::Cursor) => cursor::choice_args(choice)
            .ok_or_else(|| LaunchError::NoModelChoice(agent.label.clone())),
        Some(Profile::Pi | Profile::Omp) => Ok(pi::choice_args(choice)),
        Some(Profile::OpenCode2) | None => Err(LaunchError::NoModelChoice(agent.label.clone())),
    }
}

pub fn hook_text(taken: &[Delivery], note: Option<&str>) -> Option<String> {
    let mut parts: Vec<String> = note.map(str::to_owned).into_iter().collect();
    if !taken.is_empty() {
        parts.push(message_text(taken));
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

pub fn questions_note(open_questions: &[OpenQuestion]) -> Option<String> {
    let first = open_questions.first()?;
    let list: Vec<String> = open_questions
        .iter()
        .map(|OpenQuestion { id, text }| format!("{id}: {text}"))
        .collect();
    Some(format!(
        "Open loom ask questions: {}. If this input answers some of these questions, first call loom answered with their IDs (for example: loom answered {}). Then do the work. Do not close the other questions.",
        list.join(" | "),
        first.id
    ))
}

pub fn message_text(taken: &[Delivery]) -> String {
    taken
        .iter()
        .map(Delivery::text)
        .collect::<Vec<_>>()
        .join(crate::protocol::BATCH_SEPARATOR)
}

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

#[cfg(test)]
pub fn assert_every_launch_asks(texts: impl Fn(Launch) -> (String, String)) {
    use crate::skills::{AlwaysOn, Position};
    let off = AlwaysOn {
        o_mode: false,
        user_doc: false,
        ste_orwell: false,
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

    #[test]
    fn open_questions_become_one_note_line() {
        assert_eq!(questions_note(&[]), None);
        let note = questions_note(&[
            OpenQuestion {
                id: crate::protocol::WaitId(12),
                text: "A or B?".into(),
            },
            OpenQuestion {
                id: crate::protocol::WaitId(13),
                text: "Delete?".into(),
            },
        ])
        .unwrap();
        assert!(note.starts_with("Open loom ask questions: Q12: A or B? | Q13: Delete?. If this input answers some of these questions, first call loom answered with their IDs (for example: loom answered Q12)."), "{note}");
    }

    use super::*;

    #[test]
    fn resume_comes_first_and_codex_resume_takes_no_permission_args_but_cursor_does() {
        let claude = find(&AgentId("claude".into())).unwrap();
        let codex = find(&AgentId("codex".into())).unwrap();
        assert_eq!(
            leading_args(claude, Some("s1"), true),
            ["--resume", "s1", "--dangerously-skip-permissions"]
        );
        assert_eq!(leading_args(claude, None, false), Vec::<String>::new());
        assert_eq!(leading_args(codex, Some("t1"), true), ["resume", "t1"]);
        let cursor = find(&AgentId("cursor".into())).unwrap();
        assert_eq!(
            leading_args(cursor, Some("c1"), true),
            ["--resume", "c1", "--force", "--trust"]
        );
        assert_eq!(
            leading_args(codex, None, true),
            ["--dangerously-bypass-approvals-and-sandbox"]
        );
    }

    #[test]
    fn every_shell_dialog_has_buttons_or_a_field() {
        for asked in &SHELL {
            let line = format!("Ask {} here:", asked.text);
            let (text, shown) = screen::line(&SHELL, &format!("$ run\n{line} ")).unwrap();
            assert_eq!(text, line);
            assert_ne!(shown.field.is_some(), !shown.options.is_empty(), "{line}");
        }
        let words = screen::line(&SHELL, "Continue? [Y/n] ").unwrap().1;
        assert_eq!(words.options[0].key.bytes().unwrap(), b"y\r");
        let keys = |line: &str| -> Vec<Vec<u8>> {
            let shown = screen::line(&SHELL, line).unwrap().1;
            assert_eq!(shown.field, None, "{line}");
            shown
                .options
                .iter()
                .map(|o| o.key.bytes().unwrap())
                .collect()
        };
        assert_eq!(
            keys("Reset password? [y/n] "),
            [b"y\r".to_vec(), b"n\r".to_vec()]
        );
        assert_eq!(
            keys("Delete passphrase file? (yes/no) "),
            [b"yes\r".to_vec(), b"no\r".to_vec()]
        );
        let field = screen::line(&SHELL, "Enter passphrase for key '/k': ")
            .unwrap()
            .1;
        assert_eq!(field.field, Some(InputField::Secret));
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
            crate::open::OPENER_ENV,
        ] {
            assert!(!is_launcher_env(key), "{key}");
        }
    }

    #[test]
    fn node_terminal_keeps_colors_on() {
        let mut cmd = shell("true");
        cmd.env("NO_COLOR", "1");
        cmd.env("FORCE_COLOR", "0");
        crate::process_tree::color_terminal(&mut cmd);
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

    #[test]
    fn only_checked_agents_can_bypass() {
        let ids: Vec<_> = agents(&[])
            .into_iter()
            .filter(|a| a.can_bypass)
            .map(|a| a.id.0)
            .collect();
        assert_eq!(ids, ["claude", "codex", "opencode2", "cursor"]);
    }

    #[test]
    fn only_parts_whose_text_changed_are_stale() {
        let launch = crate::skills::Launch {
            position: crate::skills::Position::Child,
            skills: crate::skills::AlwaysOn::default(),
        };
        let mut got = start_digests(Profile::Cursor, launch);
        assert!(got.contains_key("usage/cursor"));
        assert!(changed_parts(&got).is_empty());
        got.insert("usage/cursor".into(), "old".into());
        got.insert("gone/SKILL.md".into(), "old".into());
        assert_eq!(changed_parts(&got), ["gone/SKILL.md", "usage/cursor"]);
    }
}
