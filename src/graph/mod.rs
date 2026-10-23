//! 그래프: 노드 나무와 메일함. SQLite에 두고, 바뀔 때마다 화면에 이벤트 하나를 보낸다.
//! 바뀐 일은 이벤트 기록(`events`)에도 남긴다.
//! 나무 규칙(깊이, 이름, 받는 노드 찾기)은 나무와 메시지를 함께 보므로 이 모듈 한 곳에 둔다.
//! 근거: docs/design.md 「메시징」.

mod events;
mod gate;
mod mailbox;
mod nodes;
mod waits;

use std::collections::HashMap;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::sync::{Mutex, MutexGuard};

use rusqlite::types::Type;
use rusqlite::{Connection, Row, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::futures::Notified;
use tokio::sync::{Notify, broadcast};
use ts_rs::TS;

pub use events::{AgentSwitch, Entry, EventRecord, SwitchCause, TurnCheck, Via, Who};
pub use gate::{Ended, GateChoice, MergeTask, Round};
pub use mailbox::{Delivery, Kind, MessageId, PARENT, REDELIVER_AFTER, Report, Sent};
pub use nodes::{Active, Launched, Listed, NewChild, Relations};
pub use waits::{
    HUMAN, HumanWait, OpenQuestion, Prompt, PromptKey, PromptOption, WaitDoc, WaitId, WaitKind,
    WaitSource,
};

use crate::db::Database;
use crate::profile::{AgentId, BRIEF_LIMIT_BYTES};
use crate::protocol::{IllegalStep, MessageKind};
use crate::worktree::RepoId;

/// 화면 하나가 밀려도 버티는 이벤트 수.
const EVENT_QUEUE: usize = 256;
/// 받는 노드마다 `queued` 메시지 상한. 근거: docs/design.md 「상한」.
const MAILBOX_LIMIT: u32 = 50;
/// `loom done --image`가 받는 그림 꼴. 브라우저가 `<img>`로 그린다. 받은편지함이 완료 줄의 파일을 그대로 낸다.
pub const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];
/// `loom done` 요약 상한. 근거: docs/design.md 「맥락과 크기」.
const SUMMARY_LIMIT_BYTES: usize = 1024;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NodeId(pub u32);

/// 노드 호스트가 주소 파일 이름에 쓴다.
impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum NodeState {
    Running,
    /// `summary`는 부모에게 보낸 요약이나 결과 파일 경로다.
    Done {
        summary: String,
    },
    /// `code`가 없으면 데몬이 다시 시작했을 때 프로세스가 사라졌던 노드다.
    Exited {
        code: Option<u32>,
    },
    /// `loom spawn --worktree`의 설정 스크립트가 이 코드로 끝나 에이전트를 띄우지 않았다.
    SetupFailed {
        code: u32,
    },
}

/// 카드 보기. `Standard`는 카드에 터미널이 있다. `Simple`은 카드에 머리 정보만 있고, 고른 노드의 터미널은 오른쪽 칸에만 열린다.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CardView {
    Standard,
    Simple,
}

/// 노드가 도는 자리: 저장소와 그 워크트리(묶음). `worktree`는 `git worktree list`가 적는 절대 경로다.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Place {
    pub repo: RepoId,
    pub worktree: String,
}

#[derive(Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NodeView {
    pub id: NodeId,
    pub name: String,
    /// 카탈로그 에이전트. 명령(기본 셸 포함)이면 없다.
    pub agent: Option<AgentId>,
    pub parent: Option<NodeId>,
    pub state: NodeState,
    /// 아직 전하지 않은(`queued`) 메시지 수.
    pub unread: u32,
    /// CLI가 알린 모델 id 그대로다. 명령 노드이거나 CLI가 알리지 않았으면 없다.
    pub model: Option<String>,
    /// CLI가 알린 추론 강도 그대로다. 명령 노드이거나 CLI가 알리지 않았으면 없다.
    pub effort: Option<String>,
    pub place: Place,
    /// 사이드바 북마크 영역의 차례. 작을수록 먼저 북마크했다. 북마크하지 않았으면 없다.
    pub bookmark: Option<u32>,
    /// 사람이 「⋯」 메뉴로 이 노드에 고른 카드 보기. 없으면 설정의 기본 보기를 따른다.
    pub card_view: Option<CardView>,
    /// 마지막 턴을 끝낸 답 글. 심플 카드의 상태 줄이다(목업 docs/mockups/node-card-view.html ⑧). 답 글을 받은 적이 없으면 없다.
    pub reply: Option<String>,
    /// 사람의 답·승인을 기다린다. 열린 `loom ask` 질문들(물은 차례), 그 뒤에 신호의 대기 하나. 없으면 빈 목록이다.
    /// 근거: docs/design.md 「사람 응답 대기」.
    pub waits: Vec<HumanWait>,
}

/// 프로필이 CLI에서 읽은 지금 모델과 추론 강도. 값은 CLI가 쓴 글 그대로다. 근거: docs/design.md 「에이전트 실행」.
#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct Model {
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[derive(Clone, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum GraphEvent {
    /// 연결할 때 먼저 한 번 보내는 노드 목록 전체. 화면은 가진 노드를 이 목록으로 바꾼다.
    /// 그래서 연결이 끊긴 동안 지운 노드가 화면에 남지 않는다(2026-10-20 사람 보고: 지운 노드 카드가 데몬을 다시 띄운 뒤에도 남음).
    Snapshot {
        nodes: Vec<NodeView>,
    },
    Node(Box<NodeView>),
    /// `child`와 그 부모 사이 간선으로 메시지가 `toward` 쪽으로 갔다.
    Message {
        child: NodeId,
        toward: Toward,
    },
    /// 부모·자식이 아닌 두 노드 사이로 메시지가 `from`에서 `to`로 갔다. 둘 사이에는 간선이 없다.
    Hop {
        from: NodeId,
        to: NodeId,
    },
    /// 노드를 지웠다. 자식은 부모 없이 남고 `Node` 이벤트로 따로 온다.
    Removed {
        id: NodeId,
    },
    /// 이벤트 기록에 한 줄을 더했다.
    Recorded {
        record: Box<EventRecord>,
    },
    /// 설정, 카드 자리, 카드 크기 가운데 하나가 바뀌었다. 값은 담지 않는다. 다른 화면은 세 가지를 다시 읽는다.
    ViewChanged,
}

/// 간선 위 메시지가 가는 쪽.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Toward {
    Parent,
    Child,
}

/// 노드 나무의 상한. 사람이 설정에서 바꾼다. 근거: docs/design.md 「상한」.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(default, rename_all = "camelCase")]
#[ts(export)]
pub struct TreeLimits {
    /// 사람이 띄운 노드가 깊이 0이다. 0이면 에이전트가 자식을 만들지 못한다. `None`이면 상한이 없다.
    pub max_depth: Option<u32>,
    /// 한 부모의 자식 수. `None`이면 상한이 없다.
    pub max_children: Option<u32>,
}

impl Default for TreeLimits {
    fn default() -> Self {
        Self {
            max_depth: Some(2),
            max_children: None,
        }
    }
}

/// 규칙에 맞지 않아 거부한 요청. 에이전트에게는 영어 문장(`Display`)이, 화면에는 코드와 값(JSON)이 간다.
/// 이벤트 기록에도 남기므로 읽을 수 있어야 한다(`Deserialize`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "code", content = "value", rename_all = "camelCase")]
#[ts(export)]
pub enum Refusal {
    EmptyName,
    NameTaken(String),
    /// `#`으로 시작하는 이름, 숫자만인 이름, `parent`. 받는 쪽 이름에서 노드 번호와 부모를 가리킨다.
    ReservedName(String),
    TooDeep,
    /// 값은 자식 수 상한이다.
    TooManyChildren(u32),
    BriefTooLong(#[ts(type = "number")] usize),
    SummaryTooLong(#[ts(type = "number")] usize),
    MailboxFull(String),
    /// 조정·중단의 대상이 부르는 노드의 부모나 자식이 아니다.
    NotRelated(String),
    /// `loom rm`·`loom relaunch`의 대상이 부르는 노드의 자식이 아니다.
    NotChild(String),
    /// `loom rm`의 대상이 가진 자식을 부르는 노드가 데려갈 수 없다. 값은 대상 이름과 데려갈 수 없는 자식과 이유다.
    ChildrenStay(String),
    Ended(String),
    HeldFull,
    Cycle,
    NotFound(String),
    /// 이름이 맞는 노드가 둘 이상이다. `candidates`는 `#<번호> <이름> (<저장소>/<워크트리>)`를 쉼표로 이은 글이다.
    Ambiguous {
        name: String,
        candidates: String,
    },
    /// 조정·중단을 받지 못하는 노드다. 값은 보낸 쪽이 적은 이름이다.
    NotSteerable(String),
    /// 세션을 이어 다시 띄우지 못하는 노드다(셸·명령). 값은 노드 이름이다.
    NoResume(String),
    /// `loom relaunch --agent`의 에이전트에 메시지 연동(프로필)이 없다. 값은 에이전트 id다.
    NoLink(String),
    /// 노드가 지금 다시 뜨는 중이라 에이전트 바꾸기를 받지 못한다. 값은 노드 이름이다.
    Relaunching(String),
    /// 노드를 띄울 저장소가 목록에 없다(그 사이에 뺐다).
    NoRepo,
    /// `loom ask`의 질문이 비었다.
    EmptyQuestion,
    /// 사람이 띄운 노드가 `--asked`(사람의 지시) 없이 `loom done`을 불렀다. 받은편지함 완료 줄에 지시가 보여야 한다.
    NoAsked,
    /// `loom answer`의 메시지가 나에게 온 `loom ask`가 아니다. 값은 메시지 번호다.
    NotAsk(u32),
    /// 부모가 있는 노드가 `loom done --doc`을 불렀다. 문서 링크는 사람의 받은편지함에만 있다. 부모는 경로를 읽는다.
    DocHasParent,
    /// 사람이 읽는 글(`loom ask`, 부모 없는 노드의 `loom done`)에 markdown 링크 밖의 주소·경로가 있다. 값은 그 주소·경로다.
    BareLink(String),
    /// `loom ask` 질문이 선택지를 내놓는데 표준 꼴 `(A: … / B: …, 권장: A)`가 아니다. 받은편지함이 선택 단추를 만들지 못한다.
    BadChoices,
    /// `loom spawn`의 지시문에 머리가 빠졌다. 값은 빠진 머리다.
    MissingHeads(Vec<String>),
    /// `--doc`의 파일이나 사람이 읽는 글 속 `file://` 링크의 파일이 `show.sh`를 지난 user-doc HTML 그대로가 아니다. 값은 그 경로나 주소다.
    UncheckedDoc(String),
    /// `loom done --image`의 파일이 그림 파일(`IMAGE_EXTENSIONS`)이 아니거나 없다. 값은 그 경로다.
    NotImage(String),
    /// `loom spawn`에 `--agent`도 `--role`도 없다.
    NoAgentOrRole,
    /// `loom spawn --role`의 역할이 설정에 없다. `names`는 설정의 역할 이름이다.
    UnknownRole {
        role: String,
        names: Vec<String>,
    },
    /// `loom spawn --role`의 역할에 에이전트가 없고 `--agent`도 없다.
    RoleHasNoAgent {
        role: String,
    },
    /// 그 번호의 `loom ask` 질문이 열려 있지 않다(답했거나 지웠다). 문서가 없는 질문의 문서도 이것이다.
    NoWait(WaitId),
    /// 화면에서 보낸 답이 비었다.
    EmptyAnswer,
    /// 번호 없는 `loom answered`인데 열린 질문이 여럿이다. 값은 열린 번호다.
    SeveralOpen(Vec<WaitId>),
    /// 알림에서 고른 승인 창이 지금 터미널 화면과 다르다(닫혔거나 글·선택지가 바뀌었다). 키를 치지 않았다.
    PromptChanged(WaitId),
    /// 관문 상태에서 할 수 없는 관문 호출.
    Gate(IllegalStep),
    /// 관문 호출을 관문(또는 그 검토자)이 아닌 노드가 불렀다.
    NotGate {
        branch: String,
        gate: String,
    },
    /// 이 브랜치에는 관문 차례가 없다. 담당이 먼저 `loom ready`를 부른다.
    NoRound(String),
    /// `loom ready`을 부른 노드의 워크트리가 그 브랜치에 없다. `yours`는 그 워크트리의 브랜치다(분리된 HEAD면 없다).
    NotOwner {
        branch: String,
        yours: Option<String>,
    },
    /// 담당 워크트리에 커밋하지 않은 변경이 있다. 값은 브랜치다.
    Uncommitted(String),
    /// 대상 브랜치가 담당 브랜치에 포함되지 않았다.
    NotAncestor {
        target: String,
        branch: String,
    },
    /// 담당 브랜치가 대상 브랜치와 같다. 값은 브랜치다.
    SameAsTarget(String),
    /// 같은 저장소에 프로세스가 사는 CI 담당자 노드가 없다.
    NoGate,
    /// CI 담당자 노드가 여럿이다. 값은 `#번호`를 쉼표로 이은 글이다.
    ManyGates(String),
    /// block 판정에 결함 파일이 없다.
    NoFindings,
    /// 손으로 쓴 글(`note`·`answer`·`done`)이 종류 낱말로 시작한다(`protocol::kind_word`). `verb`는 대신 쓸 명령이다.
    KindWord {
        word: String,
        verb: String,
    },
    /// 원래 체크아웃에 관문 스크립트(`scripts/gate/check.sh`, `ship.sh`)가 없다.
    NoGateScripts,
    /// 브랜치 담당이 자기 브랜치를 판정했다. 값은 브랜치다.
    OwnVerdict(String),
    /// 병합 끝 보고의 실행 번호가 지금 실행이 아니다(그 뒤 `loom merge`를 다시 불렀다).
    StaleRun {
        branch: String,
        #[ts(type = "number")]
        run: i64,
    },
    /// 담당 브랜치의 커밋이 모두 이미 대상 브랜치에 있다. 병합할 것이 없다.
    NothingToMerge {
        branch: String,
        target: String,
    },
    /// 보고한 병합 커밋이 대상 브랜치에 없거나 검토한 커밋을 담지 않는다.
    NotMerged {
        sha: String,
        target: String,
        head: String,
    },
    Git(String),
    Launch(String),
    Db(String),
}

impl From<IllegalStep> for Refusal {
    fn from(e: IllegalStep) -> Self {
        Refusal::Gate(e)
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::EmptyName => write!(f, "Give a name."),
            Refusal::NameTaken(name) => {
                write!(
                    f,
                    "{name}: a parent, sibling or child node already uses this name."
                )
            }
            Refusal::ReservedName(name) => write!(
                f,
                "{name}: this name is kept for node IDs and the parent. Use a name that has a letter and does not start with #."
            ),
            Refusal::TooDeep => write!(
                f,
                "The tree depth limit is reached. Do not make more children."
            ),
            Refusal::TooManyChildren(max) => write!(
                f,
                "You already have {max} children. Remove a finished child or give the work to a child."
            ),
            Refusal::BriefTooLong(len) => write!(
                f,
                "The body is {len} bytes. Make it {BRIEF_LIMIT_BYTES} bytes or less, and put long material in a file and send its path."
            ),
            Refusal::SummaryTooLong(len) => write!(
                f,
                "The summary is {len} bytes. Make it {SUMMARY_LIMIT_BYTES} bytes or less, or use `loom done --file <path>`."
            ),
            Refusal::MailboxFull(name) => write!(
                f,
                "The inbox of {name} has {MAILBOX_LIMIT} messages. Send again after that node reads them with `loom wait`."
            ),
            Refusal::NotRelated(name) => write!(
                f,
                "{name}: not your parent or child. Steer and interrupt go only between a parent and its children. Use `loom note` without --steer or --interrupt."
            ),
            Refusal::NotChild(name) => write!(
                f,
                "{name}: not your child. `loom rm` and `loom relaunch` act only on your own children."
            ),
            Refusal::ChildrenStay(list) => write!(
                f,
                "{list}. These children cannot move to you, so nothing changed. Remove or move them first."
            ),
            Refusal::Ended(name) => write!(f, "{name}: the node process has ended."),
            Refusal::HeldFull => write!(
                f,
                "You have no parent, and {MAILBOX_LIMIT} kept messages wait for one. Send again after a new parent adopts you."
            ),
            Refusal::Cycle => write!(
                f,
                "A node cannot have itself or its descendant as its parent."
            ),
            Refusal::NotFound(name) => write!(
                f,
                "{name}: no node has this name. Use `parent`, or a name or ID from `loom status --all`."
            ),
            Refusal::NotSteerable(name) => write!(
                f,
                "{name}: this node cannot take steer or interrupt (only Claude, Codex and OpenCode 2 nodes can). Use `loom note` without --steer or --interrupt."
            ),
            Refusal::NoResume(name) => write!(
                f,
                "{name}: this node runs a shell or command, which has no agent session to continue in a new process."
            ),
            Refusal::NoLink(agent) => write!(
                f,
                "{agent}: loom has no message link for this agent. Give claude, codex, cursor, or opencode2 with --agent."
            ),
            Refusal::Relaunching(name) => write!(
                f,
                "{name}: this node is starting again now. Run `loom relaunch --agent` again after it starts."
            ),
            Refusal::Ambiguous { name, candidates } => write!(
                f,
                "{name}: more than one node has this name: {candidates}. Use `#<id>`."
            ),
            Refusal::NoRepo => write!(
                f,
                "The repository is not in the list. Add the repository again."
            ),
            Refusal::EmptyQuestion => write!(f, "Give the question as one line."),
            Refusal::NoAsked => write!(
                f,
                "You have no parent, so the human reads this report in the inbox. Tell what the human asked: `loom done \"<what you did>\" --asked \"<the human's instruction>\" [--image <path>]...`."
            ),
            Refusal::NotAsk(id) => write!(
                f,
                "Message #{id} is not a loom ask that you received. `loom answer` answers only those. Use `loom note <target> <body>` for other messages."
            ),
            Refusal::DocHasParent => write!(
                f,
                "You have a parent, and it reads paths. Send the file with `loom done --file <path>` or put its path in the summary. Only nodes that the human started attach --doc."
            ),
            Refusal::BareLink(found) => write!(
                f,
                "The human sees only Markdown links whose target starts with https://, http:// or file:// and has no spaces and no parentheses. Write [label]({}) instead of `{found}`.",
                crate::link_text::drawable_form(found)
            ),
            Refusal::BadChoices => write!(
                f,
                "The question offers choices, but not in the standard form, so the inbox shows no buttons. End the question with: (A: <choice> / B: <choice>, recommended: A). Write the word for recommended in the human's language. A yes/no question needs no tail."
            ),
            Refusal::NoWait(wait) => write!(f, "{wait}: this question is not open."),
            Refusal::EmptyAnswer => write!(f, "Write an answer."),
            Refusal::SeveralOpen(open) => {
                let open: Vec<String> = open.iter().map(WaitId::to_string).collect();
                write!(
                    f,
                    "Open questions: {}. Name the ones this input answers, for example `loom answered {}`.",
                    open.join(", "),
                    open.join(" ")
                )
            }
            Refusal::PromptChanged(wait) => write!(
                f,
                "{wait}: the approval prompt in the terminal changed. Nothing was sent."
            ),
            Refusal::Gate(e) => e.fmt(f),
            Refusal::NotGate { branch, gate } => write!(
                f,
                "{branch}: only its gate {gate} (or a reviewer that {gate} started) can call this."
            ),
            Refusal::NoRound(branch) => write!(
                f,
                "{branch}: this branch has no gate round. Its owner calls `loom ready {branch}` first."
            ),
            Refusal::NotOwner { branch, yours } => write!(
                f,
                "{branch}: your worktree is on {}, so you do not own this branch. Only the node whose worktree is on the branch calls `loom ready`.",
                yours.as_deref().unwrap_or("a detached HEAD")
            ),
            Refusal::Uncommitted(branch) => write!(
                f,
                "{branch}: commit or remove the uncommitted changes in your worktree first (git status --short)."
            ),
            Refusal::NotAncestor { target, branch } => write!(
                f,
                "{target} is not in {branch}. Merge {target} into {branch}, run the checks, commit, then call `loom ready {branch}` again."
            ),
            Refusal::SameAsTarget(branch) => write!(
                f,
                "{branch} is the target branch itself. If you work in your coordinator's branch, report with `loom done`."
            ),
            Refusal::NoGate => write!(
                f,
                "No live node with role CI 담당자 is in this repository. Ask the root to spawn one with `loom spawn <name> --role \"CI 담당자\"`."
            ),
            Refusal::ManyGates(ids) => write!(
                f,
                "More than one live CI 담당자 node is in this repository ({ids}). Ask the root to remove the extra ones."
            ),
            Refusal::NoFindings => write!(
                f,
                "A block verdict needs the findings file: `loom verdict <branch> block --findings <file>`."
            ),
            Refusal::KindWord { word, verb } => write!(
                f,
                "Do not start a message with the kind word {word}. Instead: {verb}. loom gives each message its kind and picks the receiver."
            ),
            Refusal::OwnVerdict(branch) => write!(
                f,
                "{branch}: you own this branch, so you cannot judge it. Its gate or a reviewer gives the verdict."
            ),
            Refusal::StaleRun { branch, run } => write!(
                f,
                "{branch}: merge run {run} is not the current run. A later `loom merge` replaced it, so loom ignores this report."
            ),
            Refusal::NothingToMerge { branch, target } => write!(
                f,
                "{branch} has no commit that {target} does not have, so there is nothing to merge. Commit the work first."
            ),
            Refusal::NotMerged { sha, target, head } => write!(
                f,
                "{sha} is not a commit of {target} that contains the reviewed commit {head}. loom records only a merge it can see in git."
            ),
            Refusal::NoGateScripts => write!(
                f,
                "This repository has no {} and {} in its main checkout, so loom verdict, loom merge and loom fail do not work here. `loom ready` still sends the branch to its gate; the gate merges by hand and reports with `loom done`.",
                crate::protocol::CHECK_SCRIPT,
                crate::protocol::SHIP_SCRIPT
            ),
            Refusal::Git(e) => write!(f, "git failed: {e}"),
            Refusal::Launch(e) => write!(f, "Could not start the node: {e}"),
            Refusal::MissingHeads(missing) => write!(
                f,
                "The brief has no {} head. Give each head as a line that starts with the head and a colon (`Goal: …`) or as a markdown heading (`## Goal`). The heads are: {}.",
                missing.join(", "),
                crate::task::BRIEF_HEADS.join(", ")
            ),
            Refusal::UncheckedDoc(path) => write!(
                f,
                "{path}: --doc takes only a user-doc HTML file that passed show.sh. Write the document as HTML from the user-doc style.html, run show.sh on it, then attach it."
            ),
            Refusal::NotImage(path) => write!(
                f,
                "{path}: --image takes an image file that exists ({}).",
                IMAGE_EXTENSIONS.join(", ")
            ),
            Refusal::NoAgentOrRole => f.write_str("Give --agent or --role."),
            Refusal::UnknownRole { role, names } => write!(
                f,
                "The settings have no role '{role}'. The roles are: {}.",
                names.join(", ")
            ),
            Refusal::RoleHasNoAgent { role } => write!(
                f,
                "The role '{role}' has no agent in the settings. Give --agent."
            ),
            Refusal::Db(e) => write!(f, "DB error: {e}"),
        }
    }
}

impl From<rusqlite::Error> for Refusal {
    fn from(e: rusqlite::Error) -> Self {
        Refusal::Db(e.to_string())
    }
}

pub struct Graph {
    db: Database,
    /// 화면에 보낸 마지막 이벤트 기록 id. `db`를 잠근 뒤에만 잠근다(`lock`).
    published: Mutex<u32>,
    events: broadcast::Sender<GraphEvent>,
    /// 메시지를 넣을 때마다 `wait` 모두를 깨운다. 깬 쪽이 자기 메시지인지 DB에서 본다.
    mail: Notify,
    /// 실행 중인 노드 화면의 승인 창. PTY와 함께 사라지므로 DB에 두지 않는다. 다시 시작한 데몬은 화면을 다시 읽어 채운다.
    /// 승인 신호 대기에 붙여 보낸다. 훅이 화면보다 먼저 오든 늦게 오든 보기를 만들 때 붙으므로 순서를 맞출 일이 없다.
    /// `db`를 잠근 채 잠근다. 반대 순서로 잠그지 않는다.
    prompts: Mutex<HashMap<NodeId, Prompt>>,
}

// 「안 읽음」은 사람이 눌러 앞당길 수 있는 메시지, 곧 아직 넘기지 않은 큐 메시지만 센다.
// 조정·중단은 다음 단계에 넘어가므로 누를 일이 없다. 누른 뒤 칩이 그대로면 고장으로 보였다(2026-10-22 사람 보고 "작동을 안하는 것처럼 보임").
const VIEW_COLUMNS: &str = "SELECT n.id, n.name, n.agent, n.parent, n.state,
    (SELECT count(*) FROM messages m WHERE m.recipient = n.id AND m.delivered = 0 AND m.sent_at IS NULL
        AND m.kind = '\"queue\"'),
    n.model, n.effort, n.repo, n.worktree,
    n.bookmark, n.wait,
    (SELECT json_group_array(json(d.wait)) FROM dones d WHERE d.node = n.id),
    (SELECT json_group_array(json(q.wait)) FROM questions q WHERE q.node = n.id),
    n.card_view, n.reply
    FROM nodes n";

/// 잠근 DB. 놓을 때 아직 보내지 않은 이벤트 기록을 보낸다.
/// 기록은 바꾼 트랜잭션 안에서 쓴다. 되돌린 트랜잭션의 기록은 없으므로 보내지 않는다.
struct Db<'a> {
    conn: MutexGuard<'a, Connection>,
    published: MutexGuard<'a, u32>,
    events: &'a broadcast::Sender<GraphEvent>,
}

impl Deref for Db<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.conn
    }
}

impl DerefMut for Db<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

impl Drop for Db<'_> {
    fn drop(&mut self) {
        // 잠금을 쥔 채 보낸다. 그래야 기록 순서가 DB 순서와 같다.
        let Ok(new) = events::after(&self.conn, *self.published) else {
            return;
        };
        for record in new {
            *self.published = record.id;
            let _ = self.events.send(GraphEvent::Recorded {
                record: Box::new(record),
            });
        }
    }
}

impl Graph {
    /// 저장한 그래프를 연다. `running`이던 노드는 그대로 둔다. 프로세스가 살았는지는 데몬이 다시 붙어 보고 정한다(`active`, `lost`).
    pub fn open(db: Database) -> rusqlite::Result<Self> {
        let published = events::last_id(&db.lock())?;
        Ok(Self {
            db,
            published: Mutex::new(published),
            events: broadcast::channel(EVENT_QUEUE).0,
            mail: Notify::new(),
            prompts: Mutex::default(),
        })
    }

    /// 자리가 없는 옛 노드(DB 9단계 전)를 `home`으로 옮긴다. 데몬이 시작할 때 한 번 부른다.
    pub fn settle(&self, home: &Place) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE nodes SET repo = ?1, worktree = ?2 WHERE repo IS NULL",
            params![home.repo.0, home.worktree],
        )?;
        Ok(())
    }

    pub fn place_of(&self, id: NodeId) -> rusqlite::Result<Place> {
        place_of(&self.lock(), id)
    }

    /// 그 워크트리 안의 노드 id. 워크트리를 지울 때 함께 지운다.
    pub fn nodes_in(&self, worktree: &str) -> rusqlite::Result<Vec<NodeId>> {
        self.lock()
            .prepare("SELECT id FROM nodes WHERE worktree = ?1 ORDER BY id")?
            .query_map([worktree], |row| Ok(NodeId(row.get(0)?)))?
            .collect()
    }

    /// 지금 목록과 그 뒤의 이벤트를 빈틈없이 받는다.
    pub fn subscribe(&self) -> rusqlite::Result<(Vec<NodeView>, broadcast::Receiver<GraphEvent>)> {
        let db = self.lock();
        let prompts = self.prompts.lock().unwrap();
        let views = db
            .prepare(&format!("{VIEW_COLUMNS} ORDER BY n.id"))?
            .query_map([], |row| view_from_row(row, &prompts))?
            .collect::<rusqlite::Result<_>>()?;
        drop(prompts);
        Ok((views, self.events.subscribe()))
    }

    fn lock(&self) -> Db<'_> {
        Db {
            conn: self.db.lock(),
            published: self.published.lock().unwrap(),
            events: &self.events,
        }
    }

    /// 이벤트 콘솔을 열 때 읽는 최근 기록. 최근 것부터.
    pub fn recent_events(&self) -> rusqlite::Result<Vec<EventRecord>> {
        events::recent(&self.lock())
    }

    /// 에이전트 동사를 거절한 일을 남긴다. `reason`은 부른 쪽이 받은 거절이다. `what`은 메시지 동사만 있다.
    pub fn record_refusal(
        &self,
        from: NodeId,
        to: &str,
        how: Kind,
        what: Option<MessageKind>,
        reason: &Refusal,
        via: Option<&Via>,
    ) -> rusqlite::Result<()> {
        let db = self.lock();
        let entry = events::Entry::Refused {
            how,
            what,
            from: events::who(&db, from)?,
            to: to.to_owned(),
            reason: reason.clone(),
            via: via.cloned(),
            gate: None,
        };
        events::write(&db, &entry, None)
    }

    /// 턴 끝 검사가 노드를 다시 일하게 한 일을 남긴다.
    pub fn record_nudge(&self, node: NodeId, check: events::TurnCheck) -> rusqlite::Result<()> {
        let db = self.lock();
        let entry = events::Entry::Nudged {
            node: events::who(&db, node)?,
            check,
        };
        events::write(&db, &entry, None)
    }

    /// 메시지·노드와 무관한 이벤트(계정 전환)를 남긴다.
    pub fn record(&self, entry: &Entry) -> rusqlite::Result<()> {
        events::write(&self.lock(), entry, None)
    }

    /// 잠금을 쥔 채 부른다. 그래야 이벤트 순서가 DB 변경 순서와 같다.
    fn publish_node(&self, db: &Connection, id: NodeId) -> rusqlite::Result<()> {
        let prompts = self.prompts.lock().unwrap();
        let view = db.query_row(&format!("{VIEW_COLUMNS} WHERE n.id = ?1"), [id.0], |row| {
            view_from_row(row, &prompts)
        })?;
        drop(prompts);
        // 보는 화면이 없으면 send가 실패한다. 화면은 붙을 때 목록 전체를 받으므로 버려도 된다.
        let _ = self.events.send(GraphEvent::Node(Box::new(view)));
        Ok(())
    }

    /// 다음 메시지가 들어올 때 끝나는 future. 메일함을 보기 전에 `enable()`해야 그 사이에 온 메시지를 놓치지 않는다.
    pub fn mail_changed(&self) -> Notified<'_> {
        self.mail.notified()
    }

    /// 메일함은 그대로지만 꺼내도 되는 노드가 바뀌었다(다시 띄우기 실패). 기다리는 전달 작업이 메일함을 다시 본다.
    pub fn wake_mail(&self) {
        self.mail.notify_waiters();
    }

    /// 화면 상태(설정·자리·크기)를 쓴 뒤 부른다. 보는 화면이 없으면 버린다.
    pub fn publish_view_changed(&self) {
        let _ = self.events.send(GraphEvent::ViewChanged);
    }

    fn publish_message(&self, child: NodeId, toward: Toward) {
        let _ = self.events.send(GraphEvent::Message { child, toward });
        self.mail.notify_waiters();
    }

    fn publish_hop(&self, from: NodeId, to: NodeId) {
        let _ = self.events.send(GraphEvent::Hop { from, to });
        self.mail.notify_waiters();
    }
}

fn view_from_row(row: &Row, prompts: &HashMap<NodeId, Prompt>) -> rusqlite::Result<NodeView> {
    let id = NodeId(row.get(0)?);
    let state = from_json(row, 4)?;
    let wait: Option<String> = row.get(11)?;
    let done: String = row.get(12)?;
    let asked: String = row.get(13)?;
    let card_view: Option<String> = row.get(14)?;
    // 읽지 못하는 값(앞으로 바뀐 형식)은 대기가 없는 것으로 본다. 다음 신호가 다시 쓴다.
    let mut waits: Vec<HumanWait> = serde_json::from_str(&asked).unwrap_or_default();
    waits.sort_by_key(|w| w.id.0);
    waits.extend(
        wait.and_then(|w| serde_json::from_str::<waits::Stored>(&w).ok())
            .map(|s| HumanWait {
                prompt: (s.wait.kind == WaitKind::Approval)
                    .then(|| prompts.get(&id).cloned())
                    .flatten(),
                ..s.wait
            }),
    );
    let mut done: Vec<HumanWait> = serde_json::from_str(&done).unwrap_or_default();
    done.sort_by_key(|w| w.id.0);
    waits.extend(done);
    let waits = waits.into_iter().map(HumanWait::for_screen).collect();
    Ok(NodeView {
        id,
        name: row.get(1)?,
        agent: from_json(row, 2)?,
        parent: row.get::<_, Option<u32>>(3)?.map(NodeId),
        state,
        unread: row.get(5)?,
        model: row.get(6)?,
        effort: row.get(7)?,
        place: Place {
            repo: RepoId(row.get(8)?),
            worktree: row.get(9)?,
        },
        bookmark: row.get(10)?,
        // 읽지 못하는 값은 고르지 않은 것으로 본다.
        card_view: card_view.and_then(|v| serde_json::from_str(&v).ok()),
        reply: row.get(15)?,
        waits,
    })
}

fn to_json(value: &impl Serialize) -> String {
    serde_json::to_string(value).expect("enum 값은 늘 직렬화된다")
}

fn from_json<T: DeserializeOwned>(row: &Row, idx: usize) -> rusqlite::Result<T> {
    let text: String = row.get(idx)?;
    serde_json::from_str(&text)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
}

/// NULL이면 없다(그 열을 더하기 전의 옛 줄).
fn optional_json<T: DeserializeOwned>(row: &Row, idx: usize) -> rusqlite::Result<Option<T>> {
    let text: Option<String> = row.get(idx)?;
    text.map(|text| {
        serde_json::from_str(&text)
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
    })
    .transpose()
}

fn state_of(db: &Connection, id: NodeId) -> rusqlite::Result<NodeState> {
    db.query_row("SELECT state FROM nodes WHERE id = ?1", [id.0], |row| {
        from_json(row, 0)
    })
}

fn parent_of(db: &Connection, id: NodeId) -> rusqlite::Result<Option<NodeId>> {
    db.query_row("SELECT parent FROM nodes WHERE id = ?1", [id.0], |row| {
        Ok(row.get::<_, Option<u32>>(0)?.map(NodeId))
    })
}

fn place_of(db: &Connection, id: NodeId) -> rusqlite::Result<Place> {
    db.query_row(
        "SELECT repo, worktree FROM nodes WHERE id = ?1",
        [id.0],
        |row| {
            Ok(Place {
                repo: RepoId(row.get(0)?),
                worktree: row.get(1)?,
            })
        },
    )
}

/// 노드가 도는 자리를 사람이 읽는 글로: `<저장소 폴더>/<워크트리 폴더>`.
fn place_label(repo_path: &str, worktree: &str) -> String {
    let base = |path: &str| {
        path.rsplit(['/', '\\'])
            .find(|part| !part.is_empty())
            .unwrap_or(path)
            .to_owned()
    };
    format!("{}/{}", base(repo_path), base(worktree))
}

/// 이름이 `name`인 노드가 `ids` 여럿이라 거절한다. 다시 보낼 수 있게 후보를 번호와 자리로 적는다.
fn ambiguous(db: &Connection, name: &str, ids: &[NodeId]) -> rusqlite::Result<Refusal> {
    let mut found = db.prepare(
        "SELECT n.name, coalesce(r.path, ''), coalesce(n.worktree, '') FROM nodes n
         LEFT JOIN repos r ON r.id = n.repo WHERE n.id = ?1",
    )?;
    let candidates = ids
        .iter()
        .map(|id| {
            found.query_row([id.0], |row| {
                let place = place_label(&row.get::<_, String>(1)?, &row.get::<_, String>(2)?);
                Ok(format!("#{id} {} ({place})", row.get::<_, String>(0)?))
            })
        })
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Refusal::Ambiguous {
        name: name.to_owned(),
        candidates: candidates.join(", "),
    })
}

fn name_of(db: &Connection, id: NodeId) -> rusqlite::Result<String> {
    db.query_row("SELECT name FROM nodes WHERE id = ?1", [id.0], |row| {
        row.get(0)
    })
}

fn set_state(db: &Connection, id: NodeId, state: &NodeState) -> rusqlite::Result<()> {
    let entry = events::Entry::State {
        node: events::who(db, id)?,
        from: state_of(db, id)?,
        to: state.clone(),
    };
    db.execute(
        "UPDATE nodes SET state = ?2 WHERE id = ?1",
        params![id.0, to_json(state)],
    )?;
    events::write(db, &entry, None)
}

#[cfg(test)]
mod tests;
