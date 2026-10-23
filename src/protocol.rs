use std::fmt;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 관문 동사. 각각 `loom <동사>` 명령 하나다.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum GateVerb {
    Ready,
    Verdict,
    Merge,
    Fail,
}

impl GateVerb {
    pub const ALL: [GateVerb; 4] = [
        GateVerb::Ready,
        GateVerb::Verdict,
        GateVerb::Merge,
        GateVerb::Fail,
    ];

    pub fn command(self) -> &'static str {
        match self {
            GateVerb::Ready => "loom ready",
            GateVerb::Verdict => "loom verdict",
            GateVerb::Merge => "loom merge",
            GateVerb::Fail => "loom fail",
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
    /// 담당이 `loom ready`를 불렀다. 관문이 검토한다.
    Ready,
    /// 판정 pass. 관문이 `loom merge`를 부를 수 있다.
    Passed,
    /// `loom merge`가 검사·병합 중이다. 같은 관문이 다시 부를 수 있다(앞 실행이 보고 없이 끝났을 때).
    Merging,
    Merged,
    /// 담당이 고칠 수 없는 실패. 루트가 정한다. 담당이 고친 뒤 `loom ready`로 새 차례를 연다.
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
    Stopped,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GateCall {
    Ready,
    Verdict(Judgment),
    MergeStart,
    MergeEnd(MergeEnd),
    Fail,
}

impl GateCall {
    pub fn verb(self) -> GateVerb {
        match self {
            GateCall::Ready => GateVerb::Ready,
            GateCall::Verdict(_) => GateVerb::Verdict,
            GateCall::MergeStart | GateCall::MergeEnd(_) => GateVerb::Merge,
            GateCall::Fail => GateVerb::Fail,
        }
    }

    const ALL: [GateCall; 9] = [
        GateCall::Ready,
        GateCall::Verdict(Judgment::Pass),
        GateCall::Verdict(Judgment::Block),
        GateCall::MergeStart,
        GateCall::MergeEnd(MergeEnd::Merged),
        GateCall::MergeEnd(MergeEnd::Rework),
        GateCall::MergeEnd(MergeEnd::Failed),
        GateCall::MergeEnd(MergeEnd::Stopped),
        GateCall::Fail,
    ];
}

/// 지금 상태에서 할 수 없는 관문 호출. 에이전트는 이 글을 보고 같은 턴에 맞는 동사를 부른다.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct IllegalStep {
    pub state: GateState,
    pub verb: GateVerb,
    /// 지금 상태에서 부를 수 있는 동사.
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
                "nothing".to_owned()
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
        (S::Passed | S::Merging, GateCall::MergeStart) => Some(S::Merging),
        (S::Working | S::Ready | S::Merged | S::Failed, GateCall::MergeStart) => None,
        (S::Merging, GateCall::MergeEnd(MergeEnd::Merged)) => Some(S::Merged),
        (S::Merging, GateCall::MergeEnd(MergeEnd::Rework)) => Some(S::Working),
        (S::Merging, GateCall::MergeEnd(MergeEnd::Failed)) => Some(S::Failed),
        (S::Merging, GateCall::MergeEnd(MergeEnd::Stopped)) => Some(S::Merging),
        (S::Working | S::Ready | S::Passed | S::Merged | S::Failed, GateCall::MergeEnd(_)) => None,
        (S::Working | S::Ready | S::Passed | S::Merging, GateCall::Fail) => Some(S::Failed),
        (S::Merged | S::Failed, GateCall::Fail) => None,
    }
}

fn allowed(state: GateState) -> Vec<GateVerb> {
    GateVerb::ALL
        .into_iter()
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
    GateOrReviewer,
    Gate,
}

pub fn sender(verb: GateVerb) -> Sender {
    match verb {
        GateVerb::Ready => Sender::Owner,
        GateVerb::Verdict => Sender::GateOrReviewer,
        GateVerb::Merge | GateVerb::Fail => Sender::Gate,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Receiver {
    Gate,
    Owner,
    Root,
}

pub fn receivers(call: GateCall) -> &'static [Receiver] {
    match call {
        GateCall::Ready => &[Receiver::Gate],
        GateCall::Verdict(Judgment::Pass) => &[Receiver::Gate, Receiver::Owner],
        GateCall::Verdict(Judgment::Block) => &[Receiver::Owner, Receiver::Gate],
        GateCall::MergeStart | GateCall::MergeEnd(MergeEnd::Rework) => &[Receiver::Owner],
        GateCall::MergeEnd(MergeEnd::Stopped) => &[],
        GateCall::MergeEnd(MergeEnd::Merged | MergeEnd::Failed) | GateCall::Fail => {
            &[Receiver::Root, Receiver::Owner]
        }
    }
}

pub const KEPT_FOR_CALLER: &str =
    "No other node receives this. loom keeps it in your inbox as the gate record.";

pub fn next_step(call: GateCall, to: Receiver, branch: &str) -> String {
    match (call, to) {
        (GateCall::Ready, _) => format!(
            "Next: review the branch (or spawn a reviewer), then `loom verdict {branch} pass|block --findings <file>`."
        ),
        (GateCall::Verdict(Judgment::Pass), Receiver::Gate | Receiver::Root) => {
            format!("Next: `loom merge {branch}`.")
        }
        (GateCall::Verdict(Judgment::Pass) | GateCall::MergeStart, Receiver::Owner) => {
            "Next: do not commit to this branch until the merge ends.".to_owned()
        }
        (
            GateCall::Verdict(Judgment::Block) | GateCall::MergeEnd(MergeEnd::Rework),
            Receiver::Owner,
        ) => format!(
            "Next: fix each finding or rebut it with evidence, commit, then `loom ready {branch}`."
        ),
        (
            GateCall::Verdict(Judgment::Block) | GateCall::MergeEnd(MergeEnd::Rework),
            Receiver::Gate | Receiver::Root,
        ) => "Next: wait for the owner's next `loom ready`.".to_owned(),
        (GateCall::MergeStart, Receiver::Gate | Receiver::Root) => {
            "Next: wait for the merge result.".to_owned()
        }
        (GateCall::MergeEnd(MergeEnd::Merged), Receiver::Root) => {
            "Next: accept the result and reclaim the branch nodes and worktree.".to_owned()
        }
        (GateCall::MergeEnd(MergeEnd::Merged), Receiver::Owner | Receiver::Gate) => {
            "Next: report with `loom done`. Do not commit to this branch again.".to_owned()
        }
        (GateCall::MergeEnd(MergeEnd::Failed) | GateCall::Fail, Receiver::Root) => format!(
            "Next: decide. After the cause is fixed, the owner calls `loom ready {branch}` again."
        ),
        (GateCall::MergeEnd(MergeEnd::Stopped), _) => format!(
            "Next: fix the cause above, then `loom merge {branch}` again, or `loom fail {branch} <cause>`."
        ),
        (
            GateCall::MergeEnd(MergeEnd::Failed) | GateCall::Fail,
            Receiver::Owner | Receiver::Gate,
        ) => {
            format!(
                "Next: wait for the root's decision, then fix and call `loom ready {branch}` again."
            )
        }
    }
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
    /// 병합을 시작했거나, 다시 부르면 될 일로 멈춰 `Merging`에 남았다(`cause`가 있다).
    Merging {
        branch: String,
        head: String,
        cause: Option<String>,
        log: Option<String>,
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
    /// 데몬이 바뀐 내장 스킬로 시작했다. 살아 있는 모든 에이전트 노드가 받는다.
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
            MessageKind::Merging { .. } => "merging",
            MessageKind::Merged { .. } => "merged",
            MessageKind::Rework { .. } => "rework",
            MessageKind::Failed { .. } => "failed",
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
            | MessageKind::Merging { .. }
            | MessageKind::Merged { .. }
            | MessageKind::Rework { .. }
            | MessageKind::Failed { .. } => false,
            MessageKind::Exited { .. }
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
            | MessageKind::Merging { .. }
            | MessageKind::Merged { .. }
            | MessageKind::Rework { .. }
            | MessageKind::Failed { .. }
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
            MessageKind::Merging {
                branch,
                head,
                cause,
                log,
            }
            | MessageKind::Rework {
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

pub fn gate_kind(call: GateCall, facts: GateFacts) -> MessageKind {
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
    match call {
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
        GateCall::MergeStart | GateCall::MergeEnd(MergeEnd::Stopped) => MessageKind::Merging {
            branch,
            head,
            cause,
            log,
        },
        GateCall::MergeEnd(MergeEnd::Merged) => MessageKind::Merged {
            branch,
            head,
            sha: sha.unwrap_or_default(),
            log,
        },
        GateCall::MergeEnd(MergeEnd::Rework) => MessageKind::Rework {
            branch,
            head,
            cause,
            log,
        },
        GateCall::MergeEnd(MergeEnd::Failed) | GateCall::Fail => MessageKind::Failed {
            branch,
            head,
            cause,
            log,
        },
    }
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
        "nothing for built-in skills: loom sends rules-changed when they change. For other rules, use `loom note` without this word",
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
    word("MERGED", "`loom merge`", true, true),
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

pub const COORDINATOR_ROLE: &str = "코디";
pub const CI_ROLE: &str = "CI 담당자";

pub const CHECK_SCRIPT: &str = "scripts/gate/check.sh";
pub const SHIP_SCRIPT: &str = "scripts/gate/ship.sh";
pub const SHIP_REWORK: i32 = 10;
pub const SHIP_DEPLOY_FAILED: i32 = 11;
pub const SHIP_STOPPED: i32 = 12;

/// `loom merge` 안 audit가 찾은 일. 멈출 일(`end`)이 하나라도 있으면 병합하지 않는다.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Finding {
    /// 이 차례(마지막 `loom ready`) 뒤의 흔적이 지워졌다(이벤트 상한). 위반을 다 볼 수 없다. 담당의 다음 ready가 새 차례를 연다.
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
    /// 대상 브랜치가 담당 브랜치에 포함되지 않았다. 담당이 대상을 병합해 온다.
    TargetMoved { target: String },
    /// 판정한 커밋 뒤에 브랜치가 움직였다. 검토하지 않은 커밋은 병합하지 않는다.
    HeadMoved { reviewed: String, now: String },
}

impl Finding {
    pub fn end(&self) -> Option<MergeEnd> {
        match self {
            Finding::RawGateWord { .. }
            | Finding::TargetMoved { .. }
            | Finding::HeadMoved { .. } => Some(MergeEnd::Rework),
            Finding::RangeIncomplete { .. } => Some(MergeEnd::Failed),
            Finding::OutOfGate { .. } => None,
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
            Finding::TargetMoved { target } => write!(
                f,
                "{target} is not in the branch; merge {target} into the branch first"
            ),
            Finding::HeadMoved { reviewed, now } => write!(
                f,
                "the branch moved from the reviewed commit {reviewed} to {now}; call loom ready again for a new review"
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
    let ends: Vec<MergeEnd> = findings.iter().filter_map(Finding::end).collect();
    match ends.as_slice() {
        [] => None,
        _ if ends.contains(&MergeEnd::Failed) => Some(MergeEnd::Failed),
        _ => Some(MergeEnd::Rework),
    }
}

#[macro_export]
macro_rules! message_usage {
    () => {
        "- loom ask <question>: with no parent, you MUST ask the human anything (a question, a choice, an approval, a permission) only this way, in the one-line form of the ask-human rules, then end your turn. With a parent, it asks your parent; the answer comes as a message (kind: answer), or wait for it: loom wait --reply-to <message-id> --timeout <seconds> --json.
- loom answer <message-id> <body>: answer an ask that you received.
- loom done <summary> [--file <path>] (a relative path is in $LOOM_TASK_DIR): report each finished or interim result to your parent, also when other parts of the work continue and in a turn that a message or a notice started. Call it again after more work. Do not call it when you only answer a question in text. People read the summary on your node card: write it in the language they use with you. With no parent, add --asked <the human's instruction, short> and --image <path> per image the human gave. This is the only way to tell the human that a result is ready. Send no progress reports.
- loom note <target> <body> [--steer|--interrupt]: any other message. --steer puts it between the receiver's tool calls; --interrupt stops its current work (both only to your parent or children). If you receive an interrupt, stop your previous task. Do not start a body with a kind word (QUESTION, STATUS, READY, ...): use the verb for that kind.
- loom ready <branch> (branch owner, all committed) sends the branch to its gate. In repositories with scripts/gate/check.sh and ship.sh: loom verdict <branch> pass|block [--findings <file>] (gate or its reviewer, not the owner; without --findings, review/<branch>-<your name>.md in your task folder; block goes to the owner as rework), loom merge <branch> (gate, after pass) audits, checks, merges, pushes and reports, loom fail <branch> <cause> (gate) tells the root. loom picks receivers. loom --help has details.
"
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATES: [GateState; 6] = [
        GateState::Working,
        GateState::Ready,
        GateState::Passed,
        GateState::Merging,
        GateState::Merged,
        GateState::Failed,
    ];

    const LEGAL: [(GateState, GateCall, GateState); 17] = [
        (GateState::Working, GateCall::Ready, GateState::Ready),
        (GateState::Failed, GateCall::Ready, GateState::Ready),
        (GateState::Merged, GateCall::Ready, GateState::Ready),
        (
            GateState::Passed,
            GateCall::Verdict(Judgment::Pass),
            GateState::Passed,
        ),
        (
            GateState::Passed,
            GateCall::Verdict(Judgment::Block),
            GateState::Working,
        ),
        (
            GateState::Ready,
            GateCall::Verdict(Judgment::Pass),
            GateState::Passed,
        ),
        (
            GateState::Ready,
            GateCall::Verdict(Judgment::Block),
            GateState::Working,
        ),
        (GateState::Passed, GateCall::MergeStart, GateState::Merging),
        (GateState::Merging, GateCall::MergeStart, GateState::Merging),
        (
            GateState::Merging,
            GateCall::MergeEnd(MergeEnd::Merged),
            GateState::Merged,
        ),
        (
            GateState::Merging,
            GateCall::MergeEnd(MergeEnd::Rework),
            GateState::Working,
        ),
        (
            GateState::Merging,
            GateCall::MergeEnd(MergeEnd::Failed),
            GateState::Failed,
        ),
        (
            GateState::Merging,
            GateCall::MergeEnd(MergeEnd::Stopped),
            GateState::Merging,
        ),
        (GateState::Working, GateCall::Fail, GateState::Failed),
        (GateState::Ready, GateCall::Fail, GateState::Failed),
        (GateState::Passed, GateCall::Fail, GateState::Failed),
        (GateState::Merging, GateCall::Fail, GateState::Failed),
    ];

    #[test]
    fn every_state_and_call_follows_the_table() {
        for state in STATES {
            for call in GateCall::ALL {
                let expected = LEGAL
                    .iter()
                    .find(|(s, c, _)| *s == state && *c == call)
                    .map(|(_, _, to)| *to);
                match (step(state, call), expected) {
                    (Ok(to), Some(want)) => assert_eq!(to, want, "{state:?} {call:?}"),
                    (Err(e), None) => {
                        assert_eq!((e.state, e.verb), (state, call.verb()));
                        let mut want: Vec<GateVerb> = LEGAL
                            .iter()
                            .filter(|(s, _, _)| *s == state)
                            .map(|(_, c, _)| c.verb())
                            .collect();
                        want.dedup();
                        assert_eq!(e.allowed, want, "{state:?}");
                    }
                    (got, want) => panic!("{state:?} {call:?}: got {got:?}, want {want:?}"),
                }
            }
        }
    }

    #[test]
    fn a_refusal_names_the_state_and_the_allowed_verbs() {
        let e = step(GateState::Ready, GateCall::MergeStart).unwrap_err();
        assert_eq!(e.allowed, vec![GateVerb::Verdict, GateVerb::Fail]);
        let text = e.to_string();
        assert!(text.contains("Ready"), "{text}");
        assert!(text.contains("loom verdict, loom fail"), "{text}");
        let merging = step(GateState::Merging, GateCall::Ready).unwrap_err();
        assert!(merging.to_string().contains("Merging"), "{merging}");
    }

    #[test]
    fn kind_words_are_found_only_at_the_start_in_capitals() {
        let word = |text| kind_word(text).map(|w| w.word);
        assert_eq!(gate_word("READY 출력표준 /tmp/x"), Some("READY"));
        assert_eq!(gate_word("CI RESULT: pass"), Some("CI RESULT"));
        assert_eq!(gate_word("BLOCK: two findings"), Some("BLOCK"));
        assert_eq!(gate_word("QUESTION: READY or not?"), None);
        assert_eq!(word("QUESTION: READY or not?"), Some("QUESTION"));
        assert_eq!(word("  USER DIRECTION: keep send"), Some("USER DIRECTION"));
        assert_eq!(word("STATUS 50%"), Some("STATUS"));
        assert_eq!(word("Ready for review"), None);
        assert_eq!(word("BLOCKED on the coordinator"), None);
        assert_eq!(word("DONEE"), None);
        assert_eq!(word("gate x: ready for review"), None);
        assert_eq!(
            kind_line("작업 끝.\nMERGED abc123", |_| true).map(|w| w.word),
            Some("MERGED")
        );
        assert_eq!(
            kind_line("MERGED a\nQUESTION: x?", |w| !w.gated_only).map(|w| w.word),
            Some("QUESTION")
        );
        assert!(!kind_word("READY x").unwrap().gated_only);
        assert!(kind_word("MERGED x").unwrap().gated_only);
        assert!(kind_word("QUESTION").unwrap().verb.contains("loom ask"));
        assert!(kind_word("ANSWER").unwrap().verb.contains("loom answer"));
    }

    #[test]
    fn every_kind_word_matches_its_serde_tag() {
        let kinds = [
            MessageKind::Ask,
            MessageKind::Answer {
                question: Question::Message { id: 1 },
            },
            MessageKind::Done {
                file: None,
                warning: None,
            },
            MessageKind::Note,
            gate_kind(GateCall::Ready, facts()),
            gate_kind(GateCall::Verdict(Judgment::Pass), facts()),
            gate_kind(GateCall::MergeStart, facts()),
            gate_kind(GateCall::MergeEnd(MergeEnd::Merged), facts()),
            gate_kind(GateCall::MergeEnd(MergeEnd::Rework), facts()),
            gate_kind(GateCall::Fail, facts()),
            MessageKind::Exited {
                node: "w".into(),
                code: None,
            },
            MessageKind::SetupFailed {
                node: "w".into(),
                code: 1,
            },
            MessageKind::Removed { node: "w".into() },
            MessageKind::Adopted {
                node: "w".into(),
                parent: "p".into(),
            },
            MessageKind::HumanInput { node: "w".into() },
            MessageKind::RulesChanged {
                skills: vec!["o-mode".into()],
                commit: None,
            },
        ];
        for kind in &kinds {
            let tag = serde_json::to_value(kind).unwrap()["kind"]
                .as_str()
                .unwrap()
                .to_owned();
            let kebab: String = tag
                .chars()
                .flat_map(|c| match c.is_ascii_uppercase() {
                    true => vec!['-', c.to_ascii_lowercase()],
                    false => vec![c],
                })
                .collect();
            assert_eq!(kind.word(), kebab);
        }
        let words: std::collections::HashSet<_> = kinds.iter().map(MessageKind::word).collect();
        assert_eq!(words.len(), kinds.len(), "every kind has its own word");
    }

    fn facts() -> GateFacts {
        GateFacts {
            branch: "b".into(),
            head: "abc".into(),
            worktree: "/w".into(),
            target: "main".into(),
            findings: Some("/r.md".into()),
            reviewer: Some("claude opus".into()),
            sha: Some("def".into()),
            cause: None,
            log: None,
        }
    }

    #[test]
    fn the_terminal_head_names_the_kind_and_loom_for_notices() {
        let verdict = gate_kind(GateCall::Verdict(Judgment::Block), facts());
        assert_eq!(
            message_text(12, "CI", Some(&verdict), None, "Next: fix"),
            "loom message #12 (from: CI, kind: verdict)\nbranch: b\nhead: abc\njudgment: block\nfindings: /r.md\nreviewer: claude opus\nNext: fix"
        );
        let typed = MessageKind::HumanInput { node: "w".into() };
        assert_eq!(
            message_text(13, "w", Some(&typed), None, "keep send"),
            "loom message #13 (from: loom, kind: human-input)\nnode: w\nkeep send"
        );
        assert_eq!(
            message_text(14, "p", None, Some("steer"), "old row"),
            "loom message #14 (from: p, kind: note, mode: steer)\nold row"
        );
        let exited = MessageKind::Exited {
            node: "w".into(),
            code: Some(0),
        };
        assert_eq!(
            message_text(15, "w", Some(&exited), None, ""),
            "loom message #15 (from: loom, kind: exited)\nnode: w\ncode: 0"
        );
    }

    #[test]
    fn usage_names_every_agent_verb() {
        let usage = crate::message_usage!();
        for verb in [
            "loom ask",
            "loom answer",
            "loom done",
            "loom note",
            "--steer",
            "--interrupt",
            "loom ready",
            "loom verdict",
            "loom merge",
            "loom fail",
            "loom wait --reply-to",
        ] {
            assert!(usage.contains(verb), "{verb}");
        }
        for old in ["loom send", "loom reply", "loom steer", "loom interrupt"] {
            assert!(!crate::profile::COMMUNICATION_USAGE.contains(old), "{old}");
        }
    }

    #[test]
    fn audit_fails_when_any_finding_is_not_the_owners_to_fix() {
        let raw = Finding::RawGateWord {
            event: 1,
            from: "w".into(),
            word: "READY".into(),
        };
        let out = Finding::OutOfGate {
            commit: "abc".into(),
        };
        assert_eq!(audit_end(&[]), None);
        assert_eq!(
            audit_end(std::slice::from_ref(&raw)),
            Some(MergeEnd::Rework)
        );
        assert_eq!(audit_end(std::slice::from_ref(&out)), None);
        assert_eq!(audit_end(&[raw.clone(), out]), Some(MergeEnd::Rework));
        let gap = Finding::RangeIncomplete {
            since: 1,
            oldest: 2,
        };
        assert_eq!(audit_end(&[raw, gap]), Some(MergeEnd::Failed));
        let moved = Finding::HeadMoved {
            reviewed: "a".into(),
            now: "b".into(),
        };
        assert_eq!(audit_end(&[moved]), Some(MergeEnd::Rework));
    }

    #[test]
    fn target_commits_between_gate_merges_are_out_of_gate() {
        let known = ["aaa".to_owned(), "zzz".to_owned()];
        assert_eq!(
            out_of_gate("bbb\naaa\nccc\nzzz\n", &known),
            vec![Finding::OutOfGate {
                commit: "bbb".into()
            }]
        );
        assert!(out_of_gate("aaa\nbbb\n", &known).is_empty());
        assert!(out_of_gate("bbb\nccc\n", &known).is_empty());
    }
}
