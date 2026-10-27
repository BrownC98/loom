use std::fmt;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{IllegalStep, WaitId};

pub const BRIEF_LIMIT_BYTES: usize = 4 * 1024;
pub const MAILBOX_LIMIT: u32 = 50;
pub const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];
pub const SUMMARY_LIMIT_BYTES: usize = 1024;

/// 노드 나무의 상한. 사람이 설정에서 바꾼다. 근거: docs/design/메시징.md 「상한」.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
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
    /// `loom rm`의 대상이 가진 자식을 부르는 노드가 데려갈 수 없다. 값은 대상 이름과 데려갈 수 없는 자식과 이유다.
    ChildrenStay(String),
    /// `loom detach`의 대상이 이미 뿌리다.
    AlreadyRoot(String),
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
    /// `loom done`을 부른 노드에게 일을 끝내고 프로세스가 끝난 자식이 있다. 관문 차례가 열린 브랜치의 담당·검토자, 사람이 띄운 노드(역할 없음), `--keep`으로 남긴 자식은 빠진다. `inherited`는 그 자식을 지우면 부르는 노드가 데려오는 손자다.
    SpentChildren {
        children: Vec<String>,
        inherited: Vec<String>,
    },
    /// `loom answer`의 메시지가 나에게 온 `loom ask`가 아니다. 값은 메시지 번호다.
    NotAsk(u32),
    /// 부모가 있는 노드가 `loom done --doc`을 불렀다. 문서 링크는 사람의 받은편지함에만 있다. 부모는 경로를 읽는다.
    DocHasParent,
    /// 부모가 있는 노드가 `loom roles set`을 불렀다. 역할표는 부모 없는 노드(사람이 띄운 노드)만 바꾼다.
    RolesHaveParent,
    /// `loom roles set`의 값이 틀렸다(카탈로그에 없는 에이전트, 그 에이전트가 받지 않는 모델·강도). 값은 이유다.
    BadRoleSetting(String),
    /// 사람이 읽는 글(`loom ask`, 부모 없는 노드의 `loom done`)에 markdown 링크 밖의 주소·경로가 있다. 값은 그 주소·경로다.
    BareLink(String),
    /// `loom ask` 질문이 선택지를 내놓는데 표준 꼴 `(A: … / B: …, 권장: A)`가 아니다. 받은편지함이 선택 단추를 만들지 못한다.
    BadChoices,
    /// `--doc`의 파일이나 사람이 읽는 글 속 `file://` 링크의 파일이 `show.sh`를 지난 user-doc HTML 그대로가 아니다. 값은 그 경로나 주소다.
    UncheckedDoc(String),
    /// `loom ask --doc` 글의 문서 링크 글자가 그 문서의 `<h1>`이 아니다.
    DocLinkLabel {
        label: String,
        h1: String,
    },
    /// `loom done --image`의 파일이 그림 파일(`IMAGE_EXTENSIONS`)이 아니거나 없다. 값은 그 경로다.
    NotImage(String),
    /// `loom spawn --role`의 역할이 설정에 없다. `names`는 설정의 역할 이름이다.
    UnknownRole {
        role: String,
        names: Vec<String>,
    },
    /// `loom spawn --role`의 역할에 에이전트가 없다. 그 역할은 사람이 띄운다.
    RoleHasNoAgent {
        role: String,
    },
    /// 부모 없는 노드가 자기 워크트리도 `--review`도 없는 자식을 띄웠다. 값은 그 역할이다.
    LeadChildNeedsWorktree(String),
    /// 이 브랜치에는 이 역할의 검토자가 이미 있다. loom이 새 차례마다 그 검토자에게 보낸다. `reviewer`는 그 노드 이름이다.
    ReviewerSeated {
        branch: String,
        role: String,
        reviewer: String,
    },
    /// 병합 큐가 이 브랜치를 검사했고 지금 게시한다. 새 검토자도, 관문을 바꾸는 부모 변경도 받지 않는다. 값은 브랜치다.
    Publishing(String),
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
    /// 판정을 부를 수 없는 노드가 불렀다. 판정은 그 브랜치의 검토자만 부른다. 부모 브랜치에서 만든 브랜치는 관문인 부모만 부른다.
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
    /// 담당 브랜치가 대상 브랜치와 같다. 값은 브랜치다.
    SameAsTarget(String),
    /// 열린 관문 차례가 이미 담당 워크트리의 지금 커밋이다. 새 커밋만 차례를 갈음한다. 값은 브랜치다.
    SameHead(String),
    /// 판정이 적은 커밋이 열린 차례의 커밋이 아니다. `head`는 열린 차례의 커밋이다.
    StaleHead {
        branch: String,
        head: String,
    },
    /// block 판정에 결함 파일이 없다.
    NoFindings,
    /// 손으로 쓴 글(`note`·`answer`·`done`)이 종류 낱말로 시작한다(`protocol::kind_word`). `verb`는 대신 쓸 명령이다.
    KindWord {
        word: String,
        verb: String,
    },
    /// 브랜치 담당이 자기 브랜치를 판정했다. 값은 브랜치다.
    OwnVerdict(String),
    /// 판정한 노드가 이 브랜치의 검토자(`loom ready` 때 데몬이 띄운 노드, 또는 `loom spawn --review <브랜치>`)가 아니다. `review`는 그 노드가 검토하는 브랜치다.
    NotReviewer {
        branch: String,
        review: Option<String>,
    },
    /// 부모 브랜치에서 만든 브랜치는 검토자가 없다. 관문인 부모가 완료 조건을 보고 직접 판정한다. `gate`는 그 부모다.
    CoordinatorAccepts {
        branch: String,
        gate: String,
    },
    /// 담당 브랜치의 커밋이 모두 이미 대상 브랜치에 있다. 병합할 것이 없다.
    NothingToMerge {
        branch: String,
        target: String,
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
            Refusal::AlreadyRoot(name) => {
                write!(f, "{name}: already a root node. It has no parent.")
            }
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
            Refusal::SpentChildren {
                children,
                inherited,
            } => {
                write!(
                    f,
                    "These children finished and their processes ended: {}. Save the results that you need from each. Then remove it with `loom rm <name>`, and remove its unmerged worktree with `git worktree remove <path>`.",
                    children.join(", ")
                )?;
                if !inherited.is_empty() {
                    write!(
                        f,
                        " `loom rm` makes their children your children: {}. Clean them up too.",
                        inherited.join(", ")
                    )?;
                }
                write!(
                    f,
                    " To keep a child on purpose, add `--keep <name>=<reason>` to `loom done`. Your report shows the reason."
                )
            }
            Refusal::NotAsk(id) => write!(
                f,
                "Message #{id} is not a loom ask that you received. `loom answer` answers only those. Use `loom note <target> <body>` for other messages."
            ),
            Refusal::DocHasParent => write!(
                f,
                "You have a parent, and it reads paths. Send the file with `loom done --file <path>` or put its path in the summary. Only nodes that the human started attach --doc."
            ),
            Refusal::RolesHaveParent => write!(
                f,
                "You have a parent. Only a node that the human started changes the role table. Ask your parent with `loom ask`."
            ),
            Refusal::BadRoleSetting(reason) => write!(f, "The role setting is not valid: {reason}"),
            Refusal::BareLink(found) => write!(
                f,
                "The human sees only Markdown links whose target starts with https://, http:// or file:// and has no spaces and no parentheses. Write [label]({}) instead of `{found}`.",
                crate::link_text::drawable_form(found)
            ),
            Refusal::BadChoices => write!(
                f,
                "The question offers choices, but not in the standard form, so the inbox shows no buttons. End the question with: (A: <choice> / B: <choice>, recommended: A). Write the word for recommended in the human's language. A yes/no question also needs the tail: (A: yes / B: no, recommended: A)."
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
                "{branch}: its gate is {gate}. Only {gate} and the reviewers that loom spawned as its children handle this branch."
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
            Refusal::SameAsTarget(branch) => write!(
                f,
                "{branch} is the target branch itself. If you work in your parent's branch, report with `loom done`."
            ),
            Refusal::StaleHead { branch, head } => write!(
                f,
                "{branch}: the open round is head {head}, not the head that you gave. Review {head}, then call `loom verdict {branch} {head} pass|block`."
            ),
            Refusal::SameHead(branch) => write!(
                f,
                "{branch}: the open gate round already has this commit. Commit the change first; then `loom ready {branch}` replaces the round."
            ),
            Refusal::NoFindings => write!(
                f,
                "A block verdict needs the findings file: `loom verdict <branch> <head> block --findings <file>`."
            ),
            Refusal::KindWord { word, verb } => write!(
                f,
                "Do not start a message with the kind word {word}. Instead: {verb}. loom gives each message its kind and picks the receiver."
            ),
            Refusal::OwnVerdict(branch) => write!(
                f,
                "{branch}: you own this branch, so you cannot judge it. A reviewer of the branch gives the verdict."
            ),
            Refusal::NotReviewer { branch, review } => match review {
                Some(review) => write!(
                    f,
                    "{branch}: you review {review}, so you cannot judge {branch}. loom spawns the reviewers of {branch} at `loom ready {branch}`."
                ),
                None => write!(
                    f,
                    "{branch}: only a reviewer of {branch} judges it. loom spawns them as the gate's children at `loom ready {branch}`."
                ),
            },
            Refusal::CoordinatorAccepts { branch, gate } => write!(
                f,
                "{branch}: a branch made from its parent's branch has no reviewer. Only that parent {gate} gives the verdict, after it checks the result against the brief's done-when."
            ),
            Refusal::NothingToMerge { branch, target } => write!(
                f,
                "{branch} has no commit that {target} does not have, so there is nothing to merge. Commit the work first."
            ),
            Refusal::Git(e) => write!(f, "git failed: {e}"),
            Refusal::Launch(e) => write!(f, "Could not start the node: {e}"),
            Refusal::UncheckedDoc(path) => write!(
                f,
                "{path}: --doc takes only a user-doc HTML file that passed show.sh. Write the document as HTML from the user-doc style.html, run show.sh on it, then attach it."
            ),
            Refusal::DocLinkLabel { label, h1 } => write!(
                f,
                "The link text \"{label}\" is not the document's <h1>. Use the <h1> as the link text, and put a version or other words after the link: `[{h1}](<file>) v2 ...`."
            ),
            Refusal::NotImage(path) => write!(
                f,
                "{path}: --image takes an image file that exists ({}).",
                IMAGE_EXTENSIONS.join(", ")
            ),
            Refusal::UnknownRole { role, names } => write!(
                f,
                "The settings have no role '{role}'. The roles are: {}.",
                names.join(", ")
            ),
            Refusal::RoleHasNoAgent { role } => write!(
                f,
                "The role '{role}' has no agent in the settings. The human starts nodes of this role."
            ),
            Refusal::LeadChildNeedsWorktree(role) => write!(
                f,
                "You have no parent, so each child owns a new worktree (`loom spawn <name> --role {role} --worktree <branch>`) or reviews your own branch (`loom spawn --role <role> --review <branch>`)."
            ),
            Refusal::ReviewerSeated {
                branch,
                role,
                reviewer,
            } => write!(
                f,
                "{reviewer} already reviews {branch} as {role}. loom sends it each new round when the owner calls `loom ready {branch}`. To tell it more, use `loom note {reviewer} <body>`. If its process ended, use `loom relaunch {reviewer}`."
            ),
            Refusal::Publishing(branch) => write!(
                f,
                "The merge queue checked {branch} and publishes it now, so it takes no new reviewer and its gate cannot change. Wait for merged or rework, then try again."
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
