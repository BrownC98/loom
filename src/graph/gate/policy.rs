use crate::protocol::{GateState, GateVerb, IllegalStep, Judgment};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum MergeEnd {
    Merged,
    Rework,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum GateCall {
    Ready,
    Verdict(Judgment),
    Requeue,
    End(MergeEnd),
}

impl GateCall {
    pub(super) fn verb(self) -> GateVerb {
        match self {
            GateCall::Ready => GateVerb::Ready,
            GateCall::Verdict(_) => GateVerb::Verdict,
            GateCall::Requeue | GateCall::End(_) => GateVerb::Queue,
        }
    }

    const ALL: [GateCall; 7] = match GateCall::Ready {
        GateCall::Ready
        | GateCall::Verdict(Judgment::Pass | Judgment::Block)
        | GateCall::Requeue
        | GateCall::End(MergeEnd::Merged | MergeEnd::Rework | MergeEnd::Failed) => [
            GateCall::Ready,
            GateCall::Verdict(Judgment::Pass),
            GateCall::Verdict(Judgment::Block),
            GateCall::Requeue,
            GateCall::End(MergeEnd::Merged),
            GateCall::End(MergeEnd::Rework),
            GateCall::End(MergeEnd::Failed),
        ],
    };
}

pub(super) fn step(state: GateState, call: GateCall) -> Result<GateState, IllegalStep> {
    next(state, call).ok_or_else(|| IllegalStep {
        state,
        verb: call.verb(),
        allowed: allowed(state),
    })
}

fn next(state: GateState, call: GateCall) -> Option<GateState> {
    use GateState as S;
    match (state, call) {
        (S::Working | S::Ready | S::Passed | S::Failed | S::Merged, GateCall::Ready) => {
            Some(S::Ready)
        }
        (S::Merging, GateCall::Ready) => None,
        (S::Ready | S::Passed, GateCall::Verdict(Judgment::Pass)) => Some(S::Passed),
        (S::Ready | S::Passed, GateCall::Verdict(Judgment::Block)) => Some(S::Working),
        (S::Working | S::Merging | S::Merged | S::Failed, GateCall::Verdict(_)) => None,
        (S::Merging, GateCall::Requeue) => Some(S::Passed),
        (S::Working | S::Ready | S::Passed | S::Merged | S::Failed, GateCall::Requeue) => None,
        (S::Merging, GateCall::End(MergeEnd::Merged)) => Some(S::Merged),
        (S::Merging, GateCall::End(MergeEnd::Rework)) => Some(S::Working),
        (S::Merging, GateCall::End(MergeEnd::Failed)) => Some(S::Failed),
        (S::Working | S::Ready | S::Passed | S::Merged | S::Failed, GateCall::End(_)) => None,
    }
}

fn allowed(state: GateState) -> Vec<GateVerb> {
    let mut verbs = Vec::new();
    for call in GateCall::ALL {
        let verb = call.verb();
        if sender(verb) != Sender::Loom && next(state, call).is_some() && !verbs.contains(&verb) {
            verbs.push(verb);
        }
    }
    verbs
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Sender {
    Owner,
    Reviewer,
    Loom,
}

pub(super) fn sender(verb: GateVerb) -> Sender {
    match verb {
        GateVerb::Ready => Sender::Owner,
        GateVerb::Verdict => Sender::Reviewer,
        GateVerb::Fail | GateVerb::Queue => Sender::Loom,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Receiver {
    Gate,
    Owner,
    Root,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Target {
    Base,
    Coordinator,
}

pub(super) fn receivers(call: GateCall, target: Target) -> &'static [Receiver] {
    match (call, target) {
        (GateCall::Ready, _) => &[Receiver::Gate],
        (GateCall::Verdict(Judgment::Pass), _) => &[Receiver::Gate, Receiver::Owner],
        (GateCall::Verdict(Judgment::Block) | GateCall::End(MergeEnd::Rework), _) => {
            &[Receiver::Owner, Receiver::Gate]
        }
        (GateCall::Requeue, _) => &[],
        (GateCall::End(MergeEnd::Merged | MergeEnd::Failed), Target::Base) => {
            &[Receiver::Owner, Receiver::Gate, Receiver::Root]
        }
        (GateCall::End(MergeEnd::Merged | MergeEnd::Failed), Target::Coordinator) => {
            &[Receiver::Owner, Receiver::Gate]
        }
    }
}

pub(super) const KEPT_FOR_CALLER: &str =
    "No other node receives this. loom keeps it in your inbox as the gate record.";

pub(super) fn held_step(to: Receiver, reviewers: &[String]) -> String {
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

pub(super) fn next_step(
    call: GateCall,
    to: Receiver,
    target: Target,
    branch: &str,
    head: &str,
) -> String {
    match (call, to) {
        (GateCall::Ready, _) if target == Target::Coordinator => format!(
            "Next: accept or return the branch yourself, with no reviewer. Check the result against the done-when of its brief (acceptance, not a code review), then call `loom verdict {branch} {head} pass`, or `loom verdict {branch} {head} block --findings <file>` with what in done-when is missing. Give this head: loom refuses a verdict for another head."
        ),
        (GateCall::Ready, _) => format!(
            "Next: if the branch has no reviewer yet, spawn one as your child with `loom spawn --role <role> --review {branch}`. loom sends it this round's head. It reviews the branch and calls `loom verdict {branch} <head> pass|block --findings <file>`. loom sends each next round to the reviewers that the branch has."
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
        (GateCall::Requeue, _) => "Next: wait for the merge result.".to_owned(),
        (GateCall::End(MergeEnd::Merged), Receiver::Root | Receiver::Gate) => {
            "Next: accept the result. loom removes the branch's reviewers, its owner node, its clean worktree and the branch.".to_owned()
        }
        (GateCall::End(MergeEnd::Merged), Receiver::Owner) => {
            "Next: nothing. loom removes this node, its clean worktree and the branch now, unless the node has no parent or has children. Do not commit to this branch again.".to_owned()
        }
        (GateCall::End(MergeEnd::Failed), Receiver::Root | Receiver::Gate) => format!(
            "Next: decide. After the cause is fixed, the owner calls `loom ready {branch}` again."
        ),
        (GateCall::End(MergeEnd::Failed), Receiver::Owner) => format!(
            "Next: wait for the decision of your gate or root, then fix and call `loom ready {branch}` again."
        ),
    }
}

pub(super) fn withdrawn_step(branch: &str, state: GateState, findings: &str) -> String {
    format!(
        "Next: write the findings that you have now to {findings}, then stop reviewing {branch}. loom shows that file to the other reviewers in the next round. The round went back to {state:?}, so loom refuses a verdict for it now. loom sends you the next round (kind: ready) when the owner calls `loom ready {branch}` again."
    )
}

pub(super) struct Peer {
    pub(super) name: String,
    pub(super) findings: String,
}

pub(super) fn peers_step(branch: &str, peers: &[Peer]) -> String {
    let listed: Vec<String> = peers
        .iter()
        .map(|p| format!("{} ({})", p.name, p.findings))
        .collect();
    format!(
        "Other reviewers of {branch} and their findings files: {}. When you find a defect, send it to them at once: `loom note <reviewer> --steer \"<defect>\"`. Do not check again or report a defect that another reviewer sent you or wrote in its file.",
        listed.join(", ")
    )
}

pub(super) fn review_again_step(
    branch: &str,
    head: &str,
    judged: Option<&str>,
    findings: &str,
) -> String {
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
        "Next: a new round of {branch} started. {fixes} Compare it with your findings file {findings}, the other reviewers' findings and the owner's rebuttals. Write your findings file again, then call `loom verdict {branch} {head} pass|block`. Give this head: loom refuses a verdict for another head."
    )
}

pub(super) fn first_round_step(branch: &str, head: &str) -> String {
    format!(
        "Next: the open round of {branch} is head {head}. Review it, write your findings file, then call `loom verdict {branch} {head} pass|block`. Give this head: loom refuses a verdict for another head."
    )
}
