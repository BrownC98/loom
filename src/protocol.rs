//! loom 프로토콜: 메시지 종류(`MessageKind`)와 그 칸, 터미널 머리 줄, 종류 낱말, 관문 동사, 브랜치마다 관문 상태와 그 전이,
//! 보내는 쪽·받는 쪽 규칙, 에이전트 사용법 글. 종류는 이 파일 한 곳에만 둔다. CLI·데몬·DB·흔적·사용법 글이 여기서 가져간다. 스킬 글은 종류를 다시 적지 않고 `loom --help`를 가리킨다.
//! 근거: 출력 표준 R8 v5 1단계(2026-10-23 사람 결정 Q559 A). 관문 글을 손으로 쓰자 받는 곳이 틀리고(READY가 총괄에게 감), 형식이 갈리고(CI 결과 세 가지),
//! 흔적 없는 병합이 생겼다. 그래서 관문 일을 하는 명령이 보고도 한다.
//! 2단계(R8 v5 ⑦ 2): 모든 에이전트 출력과 loom 알림이 종류를 가진다. 본문 앞 낱말(`done: `, `QUESTION:`)로 뜻을 짐작하지 않는다.

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

/// `loom merge`가 끝난 꼴.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MergeEnd {
    Merged,
    /// 담당이 고칠 일: 충돌, 검사 실패, 대상 브랜치가 움직임, 관문 밖 관문 낱말.
    Rework,
    /// 담당이 고칠 수 없는 일: 배포·환경, 흔적 구간 빔.
    Failed,
    /// 다시 부르면 될 일(push 거절, fetch 실패, 잠금 대기 초과)이나 루트가 볼 일(관문 밖에서 이미 병합됨)로 멈췄다.
    /// `Merging`에 남아 관문이 `loom merge`를 다시 부르거나 `loom fail`한다. 게시하지 않은 병합은 남기지 않는다(재작업 3 결정 3).
    Stopped,
}

/// 관문 호출 하나. 동사와 그 결과를 함께 담아 틀린 짝(예: ready에 판정)을 만들 수 없게 한다.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GateCall {
    Ready,
    Verdict(Judgment),
    /// `loom merge`가 시작했다.
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

    /// 상태마다 허용 동사를 셀 때 쓰는 호출 모두.
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

/// 관문 상태 전이 하나. 와일드카드 갈래가 없다: 상태나 호출을 더하면 이 함수가 컴파일되지 않는다.
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
        // 병합한 브랜치 이름을 다음 작업이 다시 쓴다(AGENTS.md: 병합 뒤 브랜치를 지운다). 새 차례다.
        (S::Working | S::Failed | S::Merged, GateCall::Ready) => Some(S::Ready),
        (S::Ready | S::Passed | S::Merging, GateCall::Ready) => None,
        // 검토자가 둘이면 둘째 판정이 pass 뒤에 온다. 병합을 시작하기 전까지 받는다.
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

/// 지금 상태에서 부를 수 있는 동사. 전이 함수에서 센다. 규칙을 두 곳에 두지 않는다.
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

/// 동사를 부를 수 있는 노드.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sender {
    /// 브랜치 담당: 자기 워크트리가 그 브랜치에 있다.
    Owner,
    /// 관문 노드나 그 자식(관문이 띄운 검토자). 담당 자신은 빼다: 코디 밑 워커는 관문(코디)의 자식이다.
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

/// 관문 호출이 알리는 노드. loom이 고른다. 보내는 노드 자신은 빼고 보낸다.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Receiver {
    Gate,
    Owner,
    /// 담당 나무의 루트(총괄).
    Root,
}

/// 담당도 pass·병합·실패를 듣는다: 그동안 브랜치를 바꾸지 않고, 실패면 고친 뒤 다시 `loom ready`한다.
/// 관문이 스스로 판정해도(받는 쪽이 자기뿐이어도) 담당에게 가므로 흔적이 남는다.
pub fn receivers(call: GateCall) -> &'static [Receiver] {
    match call {
        GateCall::Ready => &[Receiver::Gate],
        GateCall::Verdict(Judgment::Pass) => &[Receiver::Gate, Receiver::Owner],
        GateCall::Verdict(Judgment::Block) => &[Receiver::Owner, Receiver::Gate],
        GateCall::MergeStart | GateCall::MergeEnd(MergeEnd::Rework) => &[Receiver::Owner],
        // 관문 자신의 일이다. 부른 관문이 기록으로 받는다(`KEPT_FOR_CALLER`).
        GateCall::MergeEnd(MergeEnd::Stopped) => &[],
        GateCall::MergeEnd(MergeEnd::Merged | MergeEnd::Failed) | GateCall::Fail => {
            &[Receiver::Root, Receiver::Owner]
        }
    }
}

/// 받을 노드가 없을 때(담당을 지웠다, 관문 자신의 멈춤) 부른 노드가 받는 메시지의 끝 줄. 성공한 관문 호출은 모두 흔적을 남긴다(검토 C6).
pub const KEPT_FOR_CALLER: &str =
    "No other node receives this. loom keeps it in your inbox as the gate record.";

/// 받는 쪽이 다음에 할 일. 메시지 끝 줄이다.
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

/// loom 알림의 보낸 쪽 이름. 터미널 머리 줄의 `from:`이다.
pub const LOOM: &str = "loom";

impl MessageKind {
    /// 터미널 머리 줄의 낱말(kebab-case). serde 이름과 같다(테스트가 막는다).
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

    /// loom이 보낸 알림이다. 머리 줄의 보낸 쪽은 `loom`이고, 에이전트는 이 종류를 보낼 수 없다.
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

    /// 머리 줄 아래 칸 줄. 없는 칸은 빼고, 칸 순서는 정의 순서다.
    pub fn fields(&self) -> Vec<(&'static str, String)> {
        let some = |name, value: &Option<String>| value.clone().map(|v| (name, v));
        let branch = |branch: &str, head: &str| {
            vec![("branch", branch.to_owned()), ("head", head.to_owned())]
        };
        let ended = |branch: &str, head: &str, cause: &Option<String>, log: &Option<String>| {
            let mut fields = vec![("branch", branch.to_owned()), ("head", head.to_owned())];
            fields.extend(some("cause", cause));
            fields.extend(some("log", log));
            fields
        };
        match self {
            MessageKind::Ask | MessageKind::Note => Vec::new(),
            MessageKind::Answer { question } => vec![("question", question.to_string())],
            MessageKind::Done { file, warning } => some("file", file)
                .into_iter()
                .chain(some("warning", warning))
                .collect(),
            MessageKind::Ready {
                branch,
                head,
                worktree,
                target,
            } => vec![
                ("branch", branch.clone()),
                ("head", head.clone()),
                ("worktree", worktree.clone()),
                ("target", target.clone()),
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
                    "judgment",
                    match judgment {
                        Judgment::Pass => "pass",
                        Judgment::Block => "block",
                    }
                    .to_owned(),
                ));
                fields.extend(some("findings", findings));
                fields.extend(some("reviewer", reviewer));
                fields
            }
            MessageKind::Merged {
                branch: b,
                head,
                sha,
                log,
            } => {
                let mut fields = branch(b, head);
                fields.push(("sha", sha.clone()));
                fields.extend(some("log", log));
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
                ("node", node.clone()),
                ("code", code.map_or("unknown".to_owned(), |c| c.to_string())),
            ],
            MessageKind::SetupFailed { node, code } => {
                vec![("node", node.clone()), ("code", code.to_string())]
            }
            MessageKind::Removed { node } | MessageKind::HumanInput { node } => {
                vec![("node", node.clone())]
            }
            MessageKind::Adopted { node, parent } => {
                vec![("node", node.clone()), ("parent", parent.clone())]
            }
            MessageKind::RulesChanged { skills, commit } => {
                let mut fields = vec![("skills", skills.join(", "))];
                fields.extend(some("commit", commit));
                fields
            }
        }
    }
}

/// 받는 노드의 터미널에 넣는 메시지 글: 머리 줄, 칸 줄, 본문.
/// `from`은 보낸 노드 이름이다. loom 알림이면 `loom`으로 바꾼다. `what`이 없는 옛 메시지는 메모로 보인다.
/// `steer`·`interrupt`가 아니면 전달 방식을 적지 않는다.
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
    // 요약 없이 파일만 준 완료는 본문이 파일 경로다. 같은 경로를 두 번 보이지 않는다.
    lines.extend(
        what.fields()
            .into_iter()
            .filter(|(name, value)| !(*name == "file" && *value == body))
            .map(|(name, value)| format!("{name}: {value}")),
    );
    if !body.is_empty() {
        lines.push(body.to_owned());
    }
    lines.join("\n")
}

/// 관문 호출이 받는 노드에 보내는 종류. 칸 값은 관문 줄과 호출이 준다.
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

/// 종류 낱말: 손으로 쓴 글(`note`·`answer`·`done`)이 이 낱말로 시작하면 거절하고 맞는 명령을 알린다(R8 v5 ⑤ 2층).
/// 대문자만 본다. "Ready for review" 같은 보통 문장은 거절하지 않는다. 턴 끝 검사가 터미널에만 쓴 줄도 이 표로 찾는다.
pub struct KindWord {
    pub word: &'static str,
    /// 대신 쓸 명령.
    pub verb: &'static str,
    /// 관문 스크립트가 있는 저장소에서만 거절한다. 그 밖의 저장소는 `loom verdict`·`loom merge`가 돌지 않아 옛 글을 쓴다.
    pub gated_only: bool,
    /// 관문 낱말. 관문 audit가 관문 동사 밖의 관문 글을 찾는다.
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

/// 긴 낱말을 먼저 둔다(`USER DIRECTION`이 `USER`보다 먼저 맞게).
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
    // `loom ready`은 관문 스크립트 없는 저장소에서도 돈다.
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

/// 글 맨 앞의 종류 낱말.
pub fn kind_word(text: &str) -> Option<&'static KindWord> {
    let text = text.trim_start();
    KIND_WORDS.iter().find(|w| {
        text.strip_prefix(w.word)
            .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
    })
}

/// 글 맨 앞의 관문 낱말. 관문 audit가 쓴다.
pub fn gate_word(text: &str) -> Option<&'static str> {
    kind_word(text).filter(|w| w.gate).map(|w| w.word)
}

/// 글의 어느 줄이든 `applies`가 받는 종류 낱말로 시작하면 그 첫 낱말. 턴 끝 검사가 터미널에만 쓴 프로토콜 줄을 찾는다.
/// 받지 않는 줄(관문 스크립트 없는 저장소의 `MERGED`)은 건너뛰고 뒤 줄을 본다.
pub fn kind_line(
    text: &str,
    mut applies: impl FnMut(&KindWord) -> bool,
) -> Option<&'static KindWord> {
    text.lines().filter_map(kind_word).find(|w| applies(w))
}

/// 관문을 고르는 역할 이름. 설정의 기본 역할(`roles::Roles::default`)과 같다.
pub const COORDINATOR_ROLE: &str = "코디";
pub const CI_ROLE: &str = "CI 담당자";

/// 저장소 관문 계약: 이 두 스크립트가 원래 체크아웃에 있어야 `loom ready`·`loom merge`가 돈다. 있는 저장소에서만 관문 낱말을 거절한다.
/// 두 스크립트는 `bash <상대 경로>`로 부른다. Windows `loom.exe`가 섞인 경로(`C:..\scripts/...`)를 만들면 bash가 찾지 못했다(2026-10-23 Codex 실측).
pub const CHECK_SCRIPT: &str = "scripts/gate/check.sh";
pub const SHIP_SCRIPT: &str = "scripts/gate/ship.sh";
/// `ship.sh <branch> <sha>`의 끝 코드. 0이면 `sha`를 병합·push·배포 확인을 마쳤고 마지막 줄이 병합 커밋이다.
/// 담당이 고칠 일(충돌, 대상이 움직여 push 거절)이면 병합을 하지 않거나 되돌리고 이 코드로 끝낸다.
pub const SHIP_REWORK: i32 = 10;
/// 병합·push는 했고 배포 확인만 실패했다. 마지막 줄이 병합 커밋이다.
pub const SHIP_DEPLOY_FAILED: i32 = 11;
/// ship.sh 끝 코드: 다시 부르면 될 일이나 루트가 볼 일로 멈췄다(push 거절, fetch 실패, 관문 밖 병합). 게시하지 않은 병합은 남기지 않았다.
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
    /// 이 일 때문에 병합을 멈추면 그 끝. 없으면 알리기만 한다.
    /// 관문 밖 커밋은 되돌릴 수 없다. 막으면 뒤의 모든 병합이 막힌다(2026-10-23 검토 P1-2, 코디 결정).
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

/// 이번 병합의 첫 부모부터 거슬러 간 대상 first-parent 커밋(`git rev-list` 출력, 새것부터 한 줄에 하나) 가운데
/// 관문 병합(`known`)을 처음 만나기 전의 것. 관문 병합을 만나지 못하면(첫 관문 병합) 없다.
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

/// 여러 audit 결과를 하나의 끝으로: 담당이 고칠 수 없는 일이 하나라도 있으면 실패, 멈출 일이 없으면 없다.
pub fn audit_end(findings: &[Finding]) -> Option<MergeEnd> {
    let ends: Vec<MergeEnd> = findings.iter().filter_map(Finding::end).collect();
    match ends.as_slice() {
        [] => None,
        _ if ends.contains(&MergeEnd::Failed) => Some(MergeEnd::Failed),
        _ => Some(MergeEnd::Rework),
    }
}

/// 에이전트 사용법 글(`profile::COMMUNICATION_USAGE`)의 메시지 종류 줄. `concat!`에 넣으려고 매크로로 둔다.
/// 종류를 더하거나 바꾸면 이 글도 바꾼다(`tests::usage_names_every_agent_verb`가 막는다).
#[macro_export]
macro_rules! message_usage {
    () => {
        "- loom ask <question>: with no parent, you MUST ask the human anything (a question, a choice, an approval, a permission) only this way, in the one-line form of the ask-human rules, then end your turn. With a parent, it asks your parent; the answer comes as a message (kind: answer), or wait for it: loom wait --reply-to <message-id> --timeout <seconds> --json.
- loom answer <message-id> <body>: answer an ask that you received.
- loom done <summary> [--file <path>] (a relative path is in $LOOM_TASK_DIR): report each finished or interim result to your parent. Call it again after more work. People read the summary on your node card: write it in the language they use with you. With no parent, add --asked <the human's instruction> and --image <path> per image the human gave. This is the only way to tell the human that a result is ready.
- loom note <target> <body> [--steer|--interrupt]: any other message. --steer puts it between the receiver's tool calls; --interrupt stops its current work (both only to your parent or children). If you receive an interrupt, stop your previous task. A body that starts with a kind word (QUESTION, ANSWER, STATUS, ESCALATION, DONE, READY, USER DIRECTION, ...) is refused; the refusal names the verb.
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

    /// 사람이 정한 전이 표(R8 v5 ④, 워커 브리프 1번). 이 표 밖의 짝은 모두 거절이다.
    const LEGAL: [(GateState, GateCall, GateState); 17] = [
        (GateState::Working, GateCall::Ready, GateState::Ready),
        (GateState::Failed, GateCall::Ready, GateState::Ready),
        // 병합한 브랜치 이름을 다시 쓴다(검토 P1-3).
        (GateState::Merged, GateCall::Ready, GateState::Ready),
        // 둘째 검토자의 판정은 병합 시작 전까지 받는다(검토 P2-7).
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
        // 다시 부르면 될 일로 멈춘 병합은 `Merging`에 남는다(재작업 3 결정 3).
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
        // `loom ready`은 관문 스크립트 없는 저장소에서도 돌므로 READY는 어디서나 거절한다.
        assert!(!kind_word("READY x").unwrap().gated_only);
        assert!(kind_word("MERGED x").unwrap().gated_only);
        // 대화 낱말과 거절 글은 맞는 동사를 알린다.
        assert!(kind_word("QUESTION").unwrap().verb.contains("loom ask"));
        assert!(kind_word("ANSWER").unwrap().verb.contains("loom answer"));
    }

    /// 머리 줄 낱말은 serde 태그의 kebab-case다. 종류를 더하면 `word`와 `by_loom`이 함께 바뀐다(빠짐없는 match).
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

    /// 사용법 글이 에이전트가 쓰는 모든 동사를 알린다. 옛 동사는 알리지 않는다.
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
        // 관문 밖 커밋은 알리기만 한다. 흔적 구간이 빈 것만 실패다.
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
        // 앞선 관문 병합이 없으면 기준이 없다.
        assert!(out_of_gate("bbb\nccc\n", &known).is_empty());
    }
}
