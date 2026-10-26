mod agent;
mod refusal;
mod waits;

use std::fmt;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use agent::{
    AgentId, InsideError, LaunchError, Model, ModelChoice, Provider, ServerLink, StartDigests,
};
pub use refusal::{
    BRIEF_LIMIT_BYTES, IMAGE_EXTENSIONS, MAILBOX_LIMIT, Refusal, SUMMARY_LIMIT_BYTES, TreeLimits,
};
pub use waits::{
    InputField, OpenQuestion, PermissionReply, Prompt, PromptKey, PromptOption, WaitDoc, WaitId,
    WaitKind, entered,
};

pub const VERSION: &str = match option_env!("LOOM_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

pub const COMMIT: Option<&str> = option_env!("LOOM_COMMIT");

#[derive(Clone, Copy, Debug)]
pub struct Versions {
    pub now: u32,
    pub oldest: u32,
}

impl Versions {
    pub fn accepts(self, version: u32) -> bool {
        (self.oldest..=self.now).contains(&version)
    }
}

pub const PTY_HOST: Versions = Versions { now: 5, oldest: 1 };
pub const NODE_CALLS: Versions = Versions { now: 1, oldest: 1 };
pub const NODE_CALLS_HEADER: &str = "loom-protocol";
pub const STREAMS: Versions = Versions { now: 2, oldest: 1 };
pub const PI_HOOK: u32 = 2;
pub const DISTRO_HOST: u32 = 1;

/// 관문 동사. `Queue`만 빼고 각각 `loom <동사>` 명령 하나다.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum GateVerb {
    Ready,
    Verdict,
    Fail,
    /// 데몬의 병합 큐가 한 일(묶음 시작, 다시 줄 세우기, 병합 끝). 에이전트 명령이 아니다(2026-10-23 사람 결정 Q697 A).
    Queue,
}

impl GateVerb {
    pub const ALL: [GateVerb; 4] = [
        GateVerb::Ready,
        GateVerb::Verdict,
        GateVerb::Fail,
        GateVerb::Queue,
    ];

    pub fn command(self) -> &'static str {
        match self {
            GateVerb::Ready => "loom ready",
            GateVerb::Verdict => "loom verdict",
            GateVerb::Fail => "loom fail",
            GateVerb::Queue => "the merge queue",
        }
    }
}

/// 브랜치 하나의 관문 상태. 관문 표에 한 줄씩 둔다. 줄이 없는 브랜치는 `Working`이다.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum GateState {
    /// 담당이 고치는 중이다(처음, 또는 반려 뒤).
    Working,
    /// 담당이 `loom ready`를 불렀다. 관문의 검토자가 검토한다.
    Ready,
    /// 판정 pass. 데몬의 병합 큐에 서 있다.
    Passed,
    /// 데몬이 이 브랜치를 묶음에 넣어 검사·병합한다.
    Merging,
    Merged,
    /// 담당이 고칠 수 없는 실패. 루트(부모 브랜치에서 만든 브랜치면 그 부모)가 정한다. 담당이 고친 뒤 `loom ready`로 새 차례를 연다.
    Failed,
}

/// 검토 판정.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, clap::ValueEnum, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum Judgment {
    Pass,
    Block,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MergeEnd {
    Merged,
    Rework,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GateCall {
    Ready,
    Verdict(Judgment),
    Fail,
    Start,
    Requeue,
    End(MergeEnd),
}

impl GateCall {
    pub fn verb(self) -> GateVerb {
        match self {
            GateCall::Ready => GateVerb::Ready,
            GateCall::Verdict(_) => GateVerb::Verdict,
            GateCall::Fail => GateVerb::Fail,
            GateCall::Start | GateCall::Requeue | GateCall::End(_) => GateVerb::Queue,
        }
    }

    const ALL: [GateCall; 9] = [
        GateCall::Ready,
        GateCall::Verdict(Judgment::Pass),
        GateCall::Verdict(Judgment::Block),
        GateCall::Fail,
        GateCall::Start,
        GateCall::Requeue,
        GateCall::End(MergeEnd::Merged),
        GateCall::End(MergeEnd::Rework),
        GateCall::End(MergeEnd::Failed),
    ];
}

/// 지금 상태에서 할 수 없는 관문 호출. 에이전트는 이 글을 보고 같은 턴에 맞는 동사를 부른다.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct IllegalStep {
    pub state: GateState,
    pub verb: GateVerb,
    /// 지금 상태에서 에이전트가 부를 수 있는 동사.
    pub allowed: Vec<GateVerb>,
}

impl fmt::Display for IllegalStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let allowed: Vec<&str> = self.allowed.iter().map(|v| v.command()).collect();
        write!(
            f,
            "The branch gate state is {:?}, so `{}` is not allowed now. Allowed next: {}.",
            self.state,
            self.verb.command(),
            if allowed.is_empty() {
                "nothing; the merge queue sends the result".to_owned()
            } else {
                allowed.join(", ")
            }
        )
    }
}

pub fn step(state: GateState, call: GateCall) -> Result<GateState, IllegalStep> {
    next(state, call).ok_or_else(|| IllegalStep {
        state,
        verb: call.verb(),
        allowed: allowed(state),
    })
}

fn next(state: GateState, call: GateCall) -> Option<GateState> {
    use GateState as S;
    match (state, call) {
        (S::Working | S::Failed | S::Merged, GateCall::Ready) => Some(S::Ready),
        (S::Ready | S::Passed | S::Merging, GateCall::Ready) => None,
        (S::Ready | S::Passed, GateCall::Verdict(Judgment::Pass)) => Some(S::Passed),
        (S::Ready | S::Passed, GateCall::Verdict(Judgment::Block)) => Some(S::Working),
        (S::Working | S::Merging | S::Merged | S::Failed, GateCall::Verdict(_)) => None,
        (S::Working | S::Ready | S::Passed, GateCall::Fail) => Some(S::Failed),
        (S::Merging | S::Merged | S::Failed, GateCall::Fail) => None,
        (S::Passed, GateCall::Start) => Some(S::Merging),
        (S::Working | S::Ready | S::Merging | S::Merged | S::Failed, GateCall::Start) => None,
        (S::Merging, GateCall::Requeue) => Some(S::Passed),
        (S::Working | S::Ready | S::Passed | S::Merged | S::Failed, GateCall::Requeue) => None,
        (S::Merging, GateCall::End(MergeEnd::Merged)) => Some(S::Merged),
        (S::Merging, GateCall::End(MergeEnd::Rework)) => Some(S::Working),
        (S::Merging, GateCall::End(MergeEnd::Failed)) => Some(S::Failed),
        (S::Working | S::Ready | S::Passed | S::Merged | S::Failed, GateCall::End(_)) => None,
    }
}

fn allowed(state: GateState) -> Vec<GateVerb> {
    GateVerb::ALL
        .into_iter()
        .filter(|verb| sender(*verb) != Sender::Loom)
        .filter(|verb| {
            GateCall::ALL
                .iter()
                .any(|call| call.verb() == *verb && next(state, *call).is_some())
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sender {
    Owner,
    Reviewer,
    Gate,
    Loom,
}

pub fn sender(verb: GateVerb) -> Sender {
    match verb {
        GateVerb::Ready => Sender::Owner,
        GateVerb::Verdict => Sender::Reviewer,
        GateVerb::Fail => Sender::Gate,
        GateVerb::Queue => Sender::Loom,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Receiver {
    Gate,
    Owner,
    Root,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    Base,
    Coordinator,
}

pub fn receivers(call: GateCall, target: Target) -> &'static [Receiver] {
    match (call, target) {
        (GateCall::Ready, _) => &[Receiver::Gate],
        (GateCall::Verdict(Judgment::Pass), _) => &[Receiver::Gate, Receiver::Owner],
        (GateCall::Verdict(Judgment::Block) | GateCall::End(MergeEnd::Rework), _) => {
            &[Receiver::Owner, Receiver::Gate]
        }
        (GateCall::Start | GateCall::Requeue, _) => &[],
        (GateCall::End(MergeEnd::Merged | MergeEnd::Failed) | GateCall::Fail, Target::Base) => {
            &[Receiver::Owner, Receiver::Gate, Receiver::Root]
        }
        (
            GateCall::End(MergeEnd::Merged | MergeEnd::Failed) | GateCall::Fail,
            Target::Coordinator,
        ) => &[Receiver::Owner, Receiver::Gate],
    }
}

pub const KEPT_FOR_CALLER: &str =
    "No other node receives this. loom keeps it in your inbox as the gate record.";

pub fn held_step(to: Receiver, reviewers: &[String]) -> String {
    let names = reviewers.join(", ");
    match to {
        Receiver::Owner => format!(
            "Next: do not commit to this branch. loom holds it until {names} judge it, or the gate removes them with `loom rm`."
        ),
        Receiver::Gate | Receiver::Root => format!(
            "Next: loom holds the branch until {names} judge it, or you remove them with `loom rm <name>`."
        ),
    }
}

pub fn next_step(call: GateCall, to: Receiver, target: Target, branch: &str) -> String {
    match (call, to) {
        (GateCall::Ready, _) if target == Target::Coordinator => format!(
            "Next: accept or return the branch yourself, with no reviewer. Check the result against the done-when of its brief (acceptance, not a code review), then call `loom verdict {branch} pass`, or `loom verdict {branch} block --findings <file>` with what in done-when is missing."
        ),
        (GateCall::Ready, _) => format!(
            "Next: if the branch has no reviewer yet, spawn one as your child with `loom spawn --role <role> --review {branch}`. It reviews the branch and calls `loom verdict {branch} pass|block --findings <file>`. loom sends each next round to the reviewers that the branch has."
        ),
        (GateCall::Verdict(Judgment::Pass), Receiver::Gate | Receiver::Root) => {
            "Next: nothing. loom queued the branch and sends the merge result (merged, rework or failed).".to_owned()
        }
        (GateCall::Verdict(Judgment::Pass), Receiver::Owner) => {
            "Next: do not commit to this branch. loom queued it and sends the merge result (merged, rework or failed).".to_owned()
        }
        (
            GateCall::Verdict(Judgment::Block) | GateCall::End(MergeEnd::Rework),
            Receiver::Owner,
        ) => format!(
            "Next: fix each finding or rebut it with evidence, commit, then `loom ready {branch}`."
        ),
        (
            GateCall::Verdict(Judgment::Block) | GateCall::End(MergeEnd::Rework),
            Receiver::Gate | Receiver::Root,
        ) => "Next: wait for the owner's next `loom ready`.".to_owned(),
        (GateCall::Start | GateCall::Requeue, _) => "Next: wait for the merge result.".to_owned(),
        (GateCall::End(MergeEnd::Merged), Receiver::Root | Receiver::Gate) => {
            "Next: accept the result. loom removes the branch's reviewers, its owner node, its clean worktree and the branch.".to_owned()
        }
        (GateCall::End(MergeEnd::Merged), Receiver::Owner) => {
            "Next: nothing. loom removes this node, its clean worktree and the branch now, unless the node has no parent or has children. Do not commit to this branch again.".to_owned()
        }
        (GateCall::End(MergeEnd::Failed) | GateCall::Fail, Receiver::Root | Receiver::Gate) => {
            format!(
                "Next: decide. After the cause is fixed, the owner calls `loom ready {branch}` again."
            )
        }
        (GateCall::End(MergeEnd::Failed) | GateCall::Fail, Receiver::Owner) => format!(
            "Next: wait for the decision of your gate or root, then fix and call `loom ready {branch}` again."
        ),
    }
}

pub fn withdrawn_step(branch: &str, state: GateState, findings: &str) -> String {
    format!(
        "Next: write the findings that you have now to {findings}, then stop reviewing {branch}. loom shows that file to the other reviewers in the next round. The round went back to {state:?}, so loom refuses a verdict for it now. loom sends you the next round (kind: ready) when the owner calls `loom ready {branch}` again."
    )
}

pub struct Peer {
    pub name: String,
    pub findings: String,
}

pub fn peers_step(branch: &str, peers: &[Peer]) -> String {
    let listed: Vec<String> = peers
        .iter()
        .map(|p| format!("{} ({})", p.name, p.findings))
        .collect();
    format!(
        "Other reviewers of {branch} and their findings files: {}. When you find a defect, send it to them at once: `loom note <reviewer> --steer \"<defect>\"`. Do not check again or report a defect that another reviewer sent you or wrote in its file.",
        listed.join(", ")
    )
}

pub fn review_again_step(branch: &str, head: &str, judged: Option<&str>, findings: &str) -> String {
    let fixes = match judged {
        None => format!("You did not judge {branch} before. Review the whole branch."),
        Some(judged) if judged == head => {
            "The head is the one that you judged last. Check only the rebuttals.".to_owned()
        }
        Some(judged) => format!(
            "The fixes since the head that you judged last are `git log {judged}..{head}`. Review only that range, not the whole branch again. If {judged} is not an ancestor of {head}, review the whole branch."
        ),
    };
    format!(
        "Next: a new round of {branch} started. {fixes} Compare it with your findings file {findings}, the other reviewers' findings and the owner's rebuttals. Write your findings file again, then call `loom verdict {branch} pass|block`."
    )
}

/// 메시지 하나의 종류와 칸. 모든 에이전트 출력(`loom ask`·`answer`·`done`·`note`·관문 동사)과 loom 알림이 하나를 가진다.
/// 본문(메시지의 `body`)은 칸이 아닌 글이다: 질문, 답, 요약, 메모, 다음 할 일 줄, 설정 출력, 사람 원문.
/// 칸 이름은 낱말 하나로 둔다. 터미널 머리 줄 아래에 `<칸>: <값>` 한 줄씩 그대로 보인다.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum MessageKind {
    /// 자식이 부모에게 묻는다. 부모 없는 노드의 `loom ask`는 사람 받은편지함으로 가고 메시지가 아니다.
    Ask,
    Answer {
        question: Question,
    },
    Done {
        /// 결과 파일(`--file`).
        file: Option<String>,
        /// 담당 브랜치가 병합되지 않았고 `loom ready`도 없다. 부모가 본다(요청 #3067: 워커 브랜치가 1.5시간 보이지 않게 기다렸다).
        warning: Option<String>,
    },
    Note,
    Ready {
        branch: String,
        head: String,
        worktree: String,
        target: String,
    },
    Verdict {
        branch: String,
        head: String,
        judgment: Judgment,
        findings: Option<String>,
        /// 판정한 노드의 실행자와 모델(`claude opus`).
        reviewer: Option<String>,
    },
    Merged {
        branch: String,
        head: String,
        /// 병합 커밋.
        sha: String,
        log: Option<String>,
    },
    Rework {
        branch: String,
        head: String,
        cause: Option<String>,
        log: Option<String>,
    },
    Failed {
        branch: String,
        head: String,
        cause: Option<String>,
        log: Option<String>,
    },
    /// 차례가 판정 전·병합 전에 `Working`이나 `Failed`로 돌아갔다. 그 차례의 검토자가 검토를 멈춘다.
    /// 근거: 거둔 차례를 검토자가 계속 검토해 `loom verdict` 거절 9건이 났다(2026-10-24 측정, 코디 #3889).
    Withdrawn {
        branch: String,
        head: String,
    },
    /// 노드 프로세스가 끝났다. 코드를 모르면(다시 붙지 못함) 없다.
    Exited {
        node: String,
        code: Option<u32>,
    },
    /// 설정 스크립트가 실패해 에이전트가 뜨지 않았다. 본문은 설정 출력의 끝이다.
    SetupFailed {
        node: String,
        code: u32,
    },
    Removed {
        node: String,
    },
    /// 노드의 부모가 바뀌었다. 옛 부모와 그 노드가 받는다.
    Adopted {
        node: String,
        parent: String,
    },
    /// 사람이 노드 터미널에 친 글. 그 노드의 부모가 받는다. 본문은 사람 원문이다.
    HumanInput {
        node: String,
    },
    /// 옛 데몬이 바뀐 내장 스킬로 시작할 때 보낸 알림. 지금 데몬은 보내지 않고 시작 글이 바뀐 노드를 턴 끝에 다시 띄운다.
    /// 옛 메시지와 이벤트 기록을 읽으려고 남긴다.
    RulesChanged {
        skills: Vec<String>,
        /// 빌드한 커밋(`LOOM_COMMIT`). 릴리스 빌드에는 없다.
        commit: Option<String>,
    },
}

/// 종류의 칸 이름. 터미널은 `word`를, 화면은 이 값의 번역을 칸 이름으로 보인다.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Field {
    Question,
    File,
    Warning,
    Branch,
    Head,
    Worktree,
    Target,
    Judgment,
    Findings,
    Reviewer,
    Sha,
    Cause,
    Log,
    Node,
    Code,
    Parent,
    Skills,
    Commit,
}

impl Field {
    pub fn word(self) -> &'static str {
        match self {
            Field::Question => "question",
            Field::File => "file",
            Field::Warning => "warning",
            Field::Branch => "branch",
            Field::Head => "head",
            Field::Worktree => "worktree",
            Field::Target => "target",
            Field::Judgment => "judgment",
            Field::Findings => "findings",
            Field::Reviewer => "reviewer",
            Field::Sha => "sha",
            Field::Cause => "cause",
            Field::Log => "log",
            Field::Node => "node",
            Field::Code => "code",
            Field::Parent => "parent",
            Field::Skills => "skills",
            Field::Commit => "commit",
        }
    }

    pub fn is_file(self) -> bool {
        match self {
            Field::File | Field::Findings | Field::Log => true,
            Field::Question
            | Field::Warning
            | Field::Branch
            | Field::Head
            | Field::Worktree
            | Field::Target
            | Field::Judgment
            | Field::Reviewer
            | Field::Sha
            | Field::Cause
            | Field::Node
            | Field::Code
            | Field::Parent
            | Field::Skills
            | Field::Commit => false,
        }
    }
}

/// `answer`가 답한 질문.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "in", rename_all = "camelCase")]
#[ts(export)]
pub enum Question {
    /// 자식의 `loom ask` 메시지 번호.
    Message { id: u32 },
    /// 받은편지함 질문 번호(`Q12`). 사람이 받은편지함에서 답했다.
    Inbox { id: u32 },
}

impl fmt::Display for Question {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Question::Message { id } => write!(f, "#{id}"),
            Question::Inbox { id } => write!(f, "Q{id}"),
        }
    }
}

pub const LOOM: &str = "loom";

impl MessageKind {
    pub fn word(&self) -> &'static str {
        match self {
            MessageKind::Ask => "ask",
            MessageKind::Answer { .. } => "answer",
            MessageKind::Done { .. } => "done",
            MessageKind::Note => "note",
            MessageKind::Ready { .. } => "ready",
            MessageKind::Verdict { .. } => "verdict",
            MessageKind::Merged { .. } => "merged",
            MessageKind::Rework { .. } => "rework",
            MessageKind::Failed { .. } => "failed",
            MessageKind::Withdrawn { .. } => "withdrawn",
            MessageKind::Exited { .. } => "exited",
            MessageKind::SetupFailed { .. } => "setup-failed",
            MessageKind::Removed { .. } => "removed",
            MessageKind::Adopted { .. } => "adopted",
            MessageKind::HumanInput { .. } => "human-input",
            MessageKind::RulesChanged { .. } => "rules-changed",
        }
    }

    pub fn by_loom(&self) -> bool {
        match self {
            MessageKind::Ask
            | MessageKind::Answer { .. }
            | MessageKind::Done { .. }
            | MessageKind::Note
            | MessageKind::Ready { .. }
            | MessageKind::Verdict { .. }
            | MessageKind::Merged { .. }
            | MessageKind::Rework { .. }
            | MessageKind::Failed { .. } => false,
            MessageKind::Withdrawn { .. }
            | MessageKind::Exited { .. }
            | MessageKind::SetupFailed { .. }
            | MessageKind::Removed { .. }
            | MessageKind::Adopted { .. }
            | MessageKind::HumanInput { .. }
            | MessageKind::RulesChanged { .. } => true,
        }
    }

    pub fn wakes(&self) -> bool {
        match self {
            MessageKind::Exited { .. }
            | MessageKind::Removed { .. }
            | MessageKind::Adopted { .. } => false,
            MessageKind::Ask
            | MessageKind::Answer { .. }
            | MessageKind::Done { .. }
            | MessageKind::Note
            | MessageKind::Ready { .. }
            | MessageKind::Verdict { .. }
            | MessageKind::Merged { .. }
            | MessageKind::Rework { .. }
            | MessageKind::Failed { .. }
            | MessageKind::Withdrawn { .. }
            | MessageKind::SetupFailed { .. }
            | MessageKind::HumanInput { .. }
            | MessageKind::RulesChanged { .. } => true,
        }
    }

    pub fn repeats_file(&self, body: &str) -> bool {
        matches!(self, MessageKind::Done { file: Some(file), .. } if file == body)
    }

    pub fn fields(&self) -> Vec<(Field, String)> {
        let some = |name, value: &Option<String>| value.clone().map(|v| (name, v));
        let branch = |branch: &str, head: &str| {
            vec![
                (Field::Branch, branch.to_owned()),
                (Field::Head, head.to_owned()),
            ]
        };
        let ended = |branch: &str, head: &str, cause: &Option<String>, log: &Option<String>| {
            let mut fields = vec![
                (Field::Branch, branch.to_owned()),
                (Field::Head, head.to_owned()),
            ];
            fields.extend(some(Field::Cause, cause));
            fields.extend(some(Field::Log, log));
            fields
        };
        match self {
            MessageKind::Ask | MessageKind::Note => Vec::new(),
            MessageKind::Answer { question } => vec![(Field::Question, question.to_string())],
            MessageKind::Done { file, warning } => some(Field::File, file)
                .into_iter()
                .chain(some(Field::Warning, warning))
                .collect(),
            MessageKind::Ready {
                branch,
                head,
                worktree,
                target,
            } => vec![
                (Field::Branch, branch.clone()),
                (Field::Head, head.clone()),
                (Field::Worktree, worktree.clone()),
                (Field::Target, target.clone()),
            ],
            MessageKind::Verdict {
                branch: b,
                head,
                judgment,
                findings,
                reviewer,
            } => {
                let mut fields = branch(b, head);
                fields.push((
                    Field::Judgment,
                    match judgment {
                        Judgment::Pass => "pass",
                        Judgment::Block => "block",
                    }
                    .to_owned(),
                ));
                fields.extend(some(Field::Findings, findings));
                fields.extend(some(Field::Reviewer, reviewer));
                fields
            }
            MessageKind::Merged {
                branch: b,
                head,
                sha,
                log,
            } => {
                let mut fields = branch(b, head);
                fields.push((Field::Sha, sha.clone()));
                fields.extend(some(Field::Log, log));
                fields
            }
            MessageKind::Withdrawn { branch: b, head } => branch(b, head),
            MessageKind::Rework {
                branch,
                head,
                cause,
                log,
            }
            | MessageKind::Failed {
                branch,
                head,
                cause,
                log,
            } => ended(branch, head, cause, log),
            MessageKind::Exited { node, code } => vec![
                (Field::Node, node.clone()),
                (
                    Field::Code,
                    code.map_or("unknown".to_owned(), |c| c.to_string()),
                ),
            ],
            MessageKind::SetupFailed { node, code } => {
                vec![(Field::Node, node.clone()), (Field::Code, code.to_string())]
            }
            MessageKind::Removed { node } | MessageKind::HumanInput { node } => {
                vec![(Field::Node, node.clone())]
            }
            MessageKind::Adopted { node, parent } => {
                vec![(Field::Node, node.clone()), (Field::Parent, parent.clone())]
            }
            MessageKind::RulesChanged { skills, commit } => {
                let mut fields = vec![(Field::Skills, skills.join(", "))];
                fields.extend(some(Field::Commit, commit));
                fields
            }
        }
    }
}

/// 메시지 전달 방식. 메모(`loom note`)만 조정·중단으로 보낸다. 근거: docs/design/메시징.md 「명령」.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Kind {
    /// 받는 노드의 턴을 끊지 않는다.
    Queue,
    /// `loom note --steer`: 턴을 멈추지 않고 다음 도구 호출 사이에 넣는다.
    Steer,
    /// `loom note --interrupt`: 지금 동작을 멈추고 바로 따르게 한다.
    Interrupt,
}

impl Kind {
    pub const ALL: &[Kind] = &[Kind::Queue, Kind::Steer, Kind::Interrupt];
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct MessageId(pub u32);

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Delivery {
    pub id: MessageId,
    pub from: String,
    pub kind: Kind,
    pub body: String,
    #[serde(default)]
    pub what: Option<MessageKind>,
}

pub const BATCH_SEPARATOR: &str = "\n\n";

impl Delivery {
    pub fn text(&self) -> String {
        let mode = match self.kind {
            Kind::Queue => None,
            Kind::Steer => Some("steer"),
            Kind::Interrupt => {
                Some("interrupt. Stop your current work and do what this message says")
            }
        };
        message_text(self.id.0, &self.from, self.what.as_ref(), mode, &self.body)
    }
}

pub fn message_text(
    id: u32,
    from: &str,
    what: Option<&MessageKind>,
    mode: Option<&str>,
    body: &str,
) -> String {
    let note = MessageKind::Note;
    let what = what.unwrap_or(&note);
    let from = if what.by_loom() { LOOM } else { from };
    let mode = mode.map(|m| format!(", mode: {m}")).unwrap_or_default();
    let mut lines = vec![format!(
        "loom message #{id} (from: {from}, kind: {}{mode})",
        what.word()
    )];
    let repeated = what.repeats_file(body);
    lines.extend(
        what.fields()
            .into_iter()
            .filter(|(name, _)| !(repeated && *name == Field::File))
            .map(|(name, value)| format!("{}: {value}", name.word())),
    );
    if !body.is_empty() {
        lines.push(body.to_owned());
    }
    lines.join("\n")
}

pub struct GateFacts {
    pub branch: String,
    pub head: String,
    pub worktree: String,
    pub target: String,
    pub findings: Option<String>,
    pub reviewer: Option<String>,
    pub sha: Option<String>,
    pub cause: Option<String>,
    pub log: Option<String>,
}

pub fn gate_kind(call: GateCall, facts: GateFacts) -> Option<MessageKind> {
    let GateFacts {
        branch,
        head,
        worktree,
        target,
        findings,
        reviewer,
        sha,
        cause,
        log,
    } = facts;
    Some(match call {
        GateCall::Ready => MessageKind::Ready {
            branch,
            head,
            worktree,
            target,
        },
        GateCall::Verdict(judgment) => MessageKind::Verdict {
            branch,
            head,
            judgment,
            findings,
            reviewer,
        },
        GateCall::Start | GateCall::Requeue => return None,
        GateCall::End(MergeEnd::Merged) => MessageKind::Merged {
            branch,
            head,
            sha: sha.unwrap_or_default(),
            log,
        },
        GateCall::End(MergeEnd::Rework) => MessageKind::Rework {
            branch,
            head,
            cause,
            log,
        },
        GateCall::End(MergeEnd::Failed) | GateCall::Fail => MessageKind::Failed {
            branch,
            head,
            cause,
            log,
        },
    })
}

pub struct KindWord {
    pub word: &'static str,
    pub verb: &'static str,
    pub gated_only: bool,
    pub gate: bool,
}

const fn word(word: &'static str, verb: &'static str, gated_only: bool, gate: bool) -> KindWord {
    KindWord {
        word,
        verb,
        gated_only,
        gate,
    }
}

const KIND_WORDS: [KindWord; 15] = [
    word("USER DIRECTION", NO_HUMAN_WORD, false, false),
    word("USER DECISION", NO_HUMAN_WORD, false, false),
    word(
        "RULE UPDATE",
        "nothing for built-in skills: loom relaunches each node whose start text changed. For other rules, use `loom note` without this word",
        false,
        false,
    ),
    word("QUESTION", "`loom ask`", false, false),
    word("ESCALATION", "`loom ask`", false, false),
    word("ANSWER", "`loom answer <message-id>`", false, false),
    word(
        "STATUS",
        "`loom done` (it also carries interim results)",
        false,
        false,
    ),
    word("DONE", "`loom done`", false, false),
    word("READY", "`loom ready`", false, true),
    word("CI RESULT", "`loom verdict` or `loom fail`", true, true),
    word(
        "MERGED",
        "nothing: the merge queue sends kind merged",
        true,
        true,
    ),
    word("REWORK", "`loom verdict`", true, true),
    word("BLOCK", "`loom verdict`", true, true),
    word("VERDICT", "`loom verdict`", true, true),
    word("GATE", "`loom --help`", true, true),
];

const NO_HUMAN_WORD: &str =
    "nothing: loom sends the human's terminal input to your parent (kind human-input)";

pub fn kind_word(text: &str) -> Option<&'static KindWord> {
    let text = text.trim_start();
    KIND_WORDS.iter().find(|w| {
        text.strip_prefix(w.word)
            .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
    })
}

pub fn gate_word(text: &str) -> Option<&'static str> {
    kind_word(text).filter(|w| w.gate).map(|w| w.word)
}

pub fn kind_line(
    text: &str,
    mut applies: impl FnMut(&KindWord) -> bool,
) -> Option<&'static KindWord> {
    text.lines().filter_map(kind_word).find(|w| applies(w))
}

pub const CHECK_SCRIPT: &str = "scripts/gate/check.sh";
pub const SHIP_SCRIPT: &str = "scripts/gate/ship.sh";
pub const SHIP_DEPLOY_FAILED: i32 = 11;

/// 병합 큐가 묶음에 넣기 전 audit가 찾은 일. 멈출 일(`end`)이 하나라도 있으면 그 브랜치를 묶음에 넣지 않는다.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Finding {
    /// 이 차례(마지막 `loom ready`) 뒤의 흔적이 지워졌다(이벤트 상한). 남은 범위만 보았다. 병합 보고에 붙여 알리기만 한다.
    RangeIncomplete {
        #[ts(type = "number")]
        since: i64,
        #[ts(type = "number")]
        oldest: i64,
    },
    /// 이 차례에 담당이나 그 자손(같은 저장소)이 관문 동사 밖(`loom note` 등)으로 관문 낱말 메시지를 보냈다.
    RawGateWord {
        /// 그 메시지의 이벤트 기록 번호. 이벤트 콘솔에서 찾는다.
        event: u32,
        from: String,
        word: String,
    },
    /// 대상 브랜치에 앞 관문 병합과 이번 병합 사이에 커밋이 있다(관문 밖 병합). 병합 보고에 붙여 알리기만 한다.
    OutOfGate { commit: String },
    /// 판정한 커밋 뒤에 브랜치가 움직였다. 검토하지 않은 커밋은 병합하지 않는다.
    HeadMoved { reviewed: String, now: String },
    /// 담당 워크트리가 판정한 커밋에 있지 않거나 추적 파일에 미커밋 변경이 있다. 워크트리가 없으면 보지 않는다.
    SourceDirty { worktree: String },
}

impl Finding {
    pub fn end(&self) -> Option<MergeEnd> {
        match self {
            Finding::RawGateWord { .. }
            | Finding::HeadMoved { .. }
            | Finding::SourceDirty { .. } => Some(MergeEnd::Rework),
            Finding::RangeIncomplete { .. } | Finding::OutOfGate { .. } => None,
        }
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Finding::RangeIncomplete { since, oldest } => write!(
                f,
                "range incomplete: the trace since {since} is pruned (oldest kept event {oldest})"
            ),
            Finding::RawGateWord { event, from, word } => write!(
                f,
                "event #{event}: {from} sent a message that starts with gate word {word} without a gate verb"
            ),
            Finding::OutOfGate { commit } => {
                write!(
                    f,
                    "commit {commit} is on the target with no gate merge trace"
                )
            }
            Finding::HeadMoved { reviewed, now } => write!(
                f,
                "the branch moved from the reviewed commit {reviewed} to {now}; call loom ready again for a new review"
            ),
            Finding::SourceDirty { worktree } => write!(
                f,
                "the owner worktree {worktree} is not clean at the reviewed commit; commit or revert, then call loom ready again"
            ),
        }
    }
}

pub fn out_of_gate(listed: &str, known: &[String]) -> Vec<Finding> {
    let listed: Vec<&str> = listed
        .lines()
        .map(str::trim)
        .filter(|sha| !sha.is_empty())
        .collect();
    let Some(last) = listed.iter().position(|sha| known.iter().any(|k| k == sha)) else {
        return Vec::new();
    };
    listed[..last]
        .iter()
        .map(|sha| Finding::OutOfGate {
            commit: (*sha).to_owned(),
        })
        .collect()
}

pub fn audit_end(findings: &[Finding]) -> Option<MergeEnd> {
    findings.iter().find_map(Finding::end)
}

#[macro_export]
macro_rules! message_usage {
    () => {
        "- loom ask <question>: with no parent, you MUST ask the human anything (a question, a choice, an approval, a permission) only this way, in the one-line form of the ask-human rules, then end your turn. With a parent, it asks your parent; the answer comes as a message (kind: answer), or wait for it: loom wait --reply-to <message-id> --timeout <seconds> --json.
- loom answer <message-id> <body>: answer an ask that you received.
- loom done <summary> [--file <path>] (a relative path is in $LOOM_TASK_DIR): report each finished or interim result to your parent, also when other parts of the work continue and in a turn that a message or a notice started. Call it again after more work. Do not call it when you only answer a question in text. People read the summary on your node card: write it in the language they use with you. With no parent, add --asked <the human's instruction, short> and --image <path> per image the human gave. This is the only way to tell the human that a result is ready. Send no progress reports.
- loom note <target> <body> [--steer|--interrupt]: any other message. --steer puts it between the receiver's tool calls; --interrupt stops its current work. If you receive an interrupt, stop your previous task. Do not start a body with a kind word (QUESTION, STATUS, READY, ...): use the verb for that kind.
- loom ready <branch> (branch owner, all committed) sends the branch to its gate. In repositories with scripts/gate/check.sh and ship.sh: loom verdict <branch> pass|block [--findings <file>] (into the original checkout: only a reviewer that the gate spawned with `loom spawn --role <role> --review <branch>` (one per role; loom names it and sends it each next round), never the owner; a branch made from its parent's branch has no reviewer: that parent checks the result against the brief's done-when and judges it; without --findings, review/<branch>-<your name>.md in your task folder; block goes to the owner as rework; pass puts the branch in the merge queue, and loom checks, merges and sends merged, rework or failed), loom fail <branch> <cause> (gate, before the queue takes the branch). The gate is your parent when your branch was made from its branch, else you. All return at once. loom picks receivers. loom --help has details.
"
    };
}
