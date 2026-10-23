//! 병합 관문 프로토콜: 관문 동사, 브랜치마다 관문 상태와 그 전이, 보내는 쪽·받는 쪽 규칙, 관문 낱말, 에이전트 사용법 글.
//! 관문 종류는 이 파일 한 곳에만 둔다. CLI·데몬·DB·흔적·사용법 글이 여기서 가져간다. 스킬 글은 종류를 다시 적지 않고 `loom --help`를 가리킨다.
//! 근거: 출력 표준 R8 v5 1단계(2026-10-23 사람 결정 Q559 A). 관문 글을 손으로 쓰자 받는 곳이 틀리고(READY가 총괄에게 감), 형식이 갈리고(CI 결과 세 가지),
//! 흔적 없는 병합이 생겼다. 그래서 관문 일을 하는 명령이 보고도 한다.

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
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, clap::ValueEnum)]
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
    /// 담당이 고칠 수 없는 일: 배포·환경, 관문 밖 병합, 흔적 구간 빔.
    Failed,
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
    const ALL: [GateCall; 8] = [
        GateCall::Ready,
        GateCall::Verdict(Judgment::Pass),
        GateCall::Verdict(Judgment::Block),
        GateCall::MergeStart,
        GateCall::MergeEnd(MergeEnd::Merged),
        GateCall::MergeEnd(MergeEnd::Rework),
        GateCall::MergeEnd(MergeEnd::Failed),
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
                "nothing (start a new branch)".to_owned()
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
        (S::Working | S::Failed, GateCall::Ready) => Some(S::Ready),
        (S::Ready | S::Passed | S::Merging | S::Merged, GateCall::Ready) => None,
        (S::Ready, GateCall::Verdict(Judgment::Pass)) => Some(S::Passed),
        (S::Ready, GateCall::Verdict(Judgment::Block)) => Some(S::Working),
        (S::Working | S::Passed | S::Merging | S::Merged | S::Failed, GateCall::Verdict(_)) => None,
        (S::Passed | S::Merging, GateCall::MergeStart) => Some(S::Merging),
        (S::Working | S::Ready | S::Merged | S::Failed, GateCall::MergeStart) => None,
        (S::Merging, GateCall::MergeEnd(MergeEnd::Merged)) => Some(S::Merged),
        (S::Merging, GateCall::MergeEnd(MergeEnd::Rework)) => Some(S::Working),
        (S::Merging, GateCall::MergeEnd(MergeEnd::Failed)) => Some(S::Failed),
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
    /// 관문 노드나 그 자식(관문이 띄운 검토자).
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
        GateCall::MergeEnd(MergeEnd::Merged | MergeEnd::Failed) | GateCall::Fail => {
            &[Receiver::Root, Receiver::Owner]
        }
    }
}

/// 관문 메시지 첫 줄. 머리 낱말은 소문자다. 손으로 쓴 관문 글(대문자 낱말)과 섞이지 않는다.
pub fn headline(call: GateCall, branch: &str) -> String {
    let what = match call {
        GateCall::Ready => "ready for review",
        GateCall::Verdict(Judgment::Pass) => "verdict pass",
        GateCall::Verdict(Judgment::Block) => "verdict block (rework)",
        GateCall::MergeStart => "merge started",
        GateCall::MergeEnd(MergeEnd::Merged) => "merged",
        GateCall::MergeEnd(MergeEnd::Rework) => "merge stopped (rework)",
        GateCall::MergeEnd(MergeEnd::Failed) | GateCall::Fail => "failed",
    };
    format!("gate {branch}: {what}")
}

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

/// 관문 낱말: 관문 글이 손으로 쓴 메시지 맨 앞에 오면 거절하고 맞는 동사를 알린다.
/// 대문자만 본다. "Ready for review" 같은 보통 문장은 거절하지 않는다.
const GATE_WORDS: [(&str, &str); 7] = [
    ("CI RESULT", "loom verdict` or `loom fail"),
    ("READY", "loom ready"),
    ("MERGED", "loom merge"),
    ("REWORK", "loom verdict"),
    ("BLOCK", "loom verdict"),
    ("VERDICT", "loom verdict"),
    ("GATE", "loom --help"),
];

/// 글 맨 앞의 관문 낱말과 그 대신 쓸 명령. `done: `(데몬이 붙인 완료 머리)는 건너뛰고 본다.
pub fn gate_word(text: &str) -> Option<(&'static str, &'static str)> {
    let text = text.trim_start();
    let text = text.strip_prefix("done:").map_or(text, str::trim_start);
    GATE_WORDS.into_iter().find(|(word, _)| {
        text.strip_prefix(word)
            .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
    })
}

/// 글의 어느 줄이든 관문 낱말로 시작하면 그 낱말. 턴 끝 검사가 터미널에만 쓴 관문 줄을 찾는다.
pub fn gate_line(text: &str) -> Option<&'static str> {
    text.lines().find_map(gate_word).map(|(word, _)| word)
}

/// 관문을 고르는 역할 이름. 설정의 기본 역할(`roles::Roles::default`)과 같다.
pub const COORDINATOR_ROLE: &str = "코디";
pub const CI_ROLE: &str = "CI 담당자";

/// 저장소 관문 계약: 이 두 스크립트가 원래 체크아웃에 있어야 `loom merge`가 돈다. 있는 저장소에서만 관문 낱말을 거절한다.
pub const CHECK_SCRIPT: &str = "scripts/gate/check.sh";
pub const SHIP_SCRIPT: &str = "scripts/gate/ship.sh";
/// `ship.sh <branch> <worktree>`의 끝 코드. 0이면 병합·push·배포 확인을 마쳤고 마지막 줄이 병합 커밋이다.
/// 담당이 고칠 일(충돌, main이 움직여 push 거절)이면 아무것도 바꾸지 않고 이 코드로 끝낸다.
pub const SHIP_REWORK: i32 = 10;
/// 병합·push는 했고 배포 확인만 실패했다. 마지막 줄이 병합 커밋이다.
pub const SHIP_DEPLOY_FAILED: i32 = 11;

/// `loom merge` 안 audit가 찾은 일. 하나라도 있으면 병합하지 않는다.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Finding {
    /// 앞 관문 뒤의 흔적이 지워졌다(이벤트 상한). 위반을 다 볼 수 없다.
    RangeIncomplete {
        #[ts(type = "number")]
        since: i64,
        #[ts(type = "number")]
        oldest: i64,
    },
    /// 담당 나무의 노드가 관문 동사 밖(`loom send` 등)으로 관문 낱말 메시지를 보냈다.
    RawGateWord {
        /// 그 메시지의 이벤트 기록 번호. 이벤트 콘솔에서 찾는다.
        event: u32,
        from: String,
        word: String,
    },
    /// 대상 브랜치에 관문 흔적 없는 커밋이 있다(관문 밖 병합).
    OutOfGate { commit: String },
    /// 대상 브랜치가 담당 브랜치에 포함되지 않았다. 담당이 대상을 병합해 온다.
    TargetMoved { target: String },
}

impl Finding {
    pub fn end(&self) -> MergeEnd {
        match self {
            Finding::RawGateWord { .. } | Finding::TargetMoved { .. } => MergeEnd::Rework,
            Finding::RangeIncomplete { .. } | Finding::OutOfGate { .. } => MergeEnd::Failed,
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
        }
    }
}

/// 대상 브랜치의 first-parent 커밋(`git rev-list` 출력, 한 줄에 하나) 가운데 관문이 병합하지 않은 것.
pub fn out_of_gate(listed: &str, merged: &[String]) -> Vec<Finding> {
    listed
        .lines()
        .map(str::trim)
        .filter(|sha| !sha.is_empty() && !merged.iter().any(|m| m == sha))
        .map(|sha| Finding::OutOfGate {
            commit: sha.to_owned(),
        })
        .collect()
}

/// 여러 audit 결과를 하나의 끝으로: 담당이 고칠 수 없는 일이 하나라도 있으면 실패다.
pub fn audit_end(findings: &[Finding]) -> Option<MergeEnd> {
    let ends: Vec<MergeEnd> = findings.iter().map(Finding::end).collect();
    match ends.as_slice() {
        [] => None,
        _ if ends.contains(&MergeEnd::Failed) => Some(MergeEnd::Failed),
        _ => Some(MergeEnd::Rework),
    }
}

/// 에이전트 사용법 글(`profile::COMMUNICATION_USAGE`)의 관문 줄. `concat!`에 넣으려고 매크로로 둔다.
#[macro_export]
macro_rules! gate_usage {
    () => {
        "- Merge gate (only these verbs; never write READY, MERGED, BLOCK, REWORK, VERDICT or CI RESULT in loom send/reply/done): loom ready <branch> (branch owner, all committed) sends the branch to its gate. loom verdict <branch> pass|block --findings <file> (gate or its reviewer; block needs findings and goes to the owner as rework). loom merge <branch> (gate, after pass) audits, checks, merges, pushes and reports. loom fail <branch> <cause> (gate) stops the branch and tells the root. loom picks receivers. loom --help has details.\n"
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
    const LEGAL: [(GateState, GateCall, GateState); 13] = [
        (GateState::Working, GateCall::Ready, GateState::Ready),
        (GateState::Failed, GateCall::Ready, GateState::Ready),
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
        let merged = step(GateState::Merged, GateCall::Ready).unwrap_err();
        assert!(merged.allowed.is_empty());
        assert!(merged.to_string().contains("start a new branch"));
    }

    #[test]
    fn gate_words_are_found_only_at_the_start_in_capitals() {
        assert_eq!(gate_word("READY 출력표준 /tmp/x").unwrap().0, "READY");
        assert_eq!(gate_word("  done: READY x").unwrap().0, "READY");
        assert_eq!(gate_word("CI RESULT: pass").unwrap().0, "CI RESULT");
        assert_eq!(gate_word("BLOCK: two findings").unwrap().0, "BLOCK");
        assert_eq!(gate_word("Ready for review"), None);
        assert_eq!(gate_word("BLOCKED on the coordinator"), None);
        assert_eq!(gate_word("QUESTION: READY or not?"), None);
        assert_eq!(gate_word("gate x: ready for review"), None);
        assert_eq!(gate_line("작업 끝.\nMERGED abc123"), Some("MERGED"));
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
        assert_eq!(audit_end(&[raw, out]), Some(MergeEnd::Failed));
    }

    #[test]
    fn target_commits_without_a_gate_merge_are_out_of_gate() {
        let merged = vec!["aaa".to_owned()];
        assert_eq!(
            out_of_gate("bbb\naaa\n", &merged),
            vec![Finding::OutOfGate {
                commit: "bbb".into()
            }]
        );
        assert!(out_of_gate("aaa\n", &merged).is_empty());
        assert!(out_of_gate("", &merged).is_empty());
    }
}
