use api::graph::messages::{GateState, GateVerb, IllegalStep, Judgment};

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Lineage {
    Root,
    Child,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Arrival {
    Wakes,
    Quiet,
    Finishes,
}

pub(super) fn arrival(call: GateCall, to: Receiver, target: Target, lineage: Lineage) -> Arrival {
    match (call, to, target, lineage) {
        (GateCall::End(MergeEnd::Merged), Receiver::Owner, Target::Coordinator, _) => {
            Arrival::Finishes
        }
        (GateCall::Verdict(Judgment::Pass), ..)
        | (
            GateCall::Verdict(Judgment::Block) | GateCall::End(MergeEnd::Rework),
            Receiver::Gate | Receiver::Root,
            ..,
        )
        | (GateCall::End(MergeEnd::Merged), Receiver::Owner, Target::Base, Lineage::Child) => {
            Arrival::Quiet
        }
        _ => Arrival::Wakes,
    }
}

pub(super) fn held_step(to: Receiver, branch: &str, held: &Held) -> String {
    let mut lines = Vec::new();
    if !held.reviewers.is_empty() {
        let names = held.reviewers.join(", ");
        let wait = match to {
            Receiver::Owner => {
                format!("Next: do not commit to this branch. loom holds it until {names} judge it.")
            }
            Receiver::Gate | Receiver::Root => {
                format!("Next: loom holds the branch until {names} judge it.")
            }
        };
        lines.push(match (held.stopped, to) {
            (false, _) => wait,
            (true, Receiver::Owner) => format!(
                "{wait} A reviewer stopped: the gate relaunches it with `loom relaunch <name>`, or removes it with `loom rm <name>` and you call `loom ready {branch}` again."
            ),
            (true, Receiver::Gate | Receiver::Root) => format!(
                "{wait} A reviewer stopped: `loom relaunch <name>`, or `loom rm <name>` and the owner calls `loom ready {branch}` again."
            ),
        });
    }
    if !held.seats.is_empty() {
        lines.push(format!(
            "Next: loom holds the branch until a reviewer in each empty seat ({}) passes this head. Call `loom ready {branch}` again to spawn them.",
            held.seats.join(", ")
        ));
    }
    lines.join("\n")
}

#[derive(Default)]
pub(super) struct Held {
    pub(super) reviewers: Vec<String>,
    pub(super) stopped: bool,
    pub(super) seats: Vec<&'static str>,
}

impl Held {
    pub(super) fn needs_gate(&self) -> bool {
        self.stopped || !self.seats.is_empty()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.reviewers.is_empty() && self.seats.is_empty()
    }
}

pub(super) fn next_step(
    call: GateCall,
    to: Receiver,
    target: Target,
    branch: &str,
    head: &str,
) -> String {
    let head = short(head);
    match (call, to) {
        (GateCall::Ready, _) if target == Target::Coordinator => format!(
            "Next: you judge it, with no reviewer. Check the result against its brief's done-when (acceptance, not a code review), then `loom verdict {branch} {head} pass`, or `block --findings <file>` with the missing done-when items."
        ),
        (GateCall::Ready, _) => format!(
            "Next: nothing. loom spawns the reviewers of {branch} as your children and sends them each round."
        ),
        (GateCall::Verdict(Judgment::Pass), Receiver::Gate | Receiver::Root) => {
            "Next: nothing. loom queued the branch and sends the merge result.".to_owned()
        }
        (GateCall::Verdict(Judgment::Pass), Receiver::Owner) => {
            "Next: do not commit to this branch. loom queued it and sends the merge result.".to_owned()
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
        (GateCall::End(MergeEnd::Merged), Receiver::Root | Receiver::Gate)
            if target == Target::Coordinator =>
        {
            "Next: accept the result. loom keeps the owner node until your branch is merged.".to_owned()
        }
        (GateCall::End(MergeEnd::Merged), Receiver::Root | Receiver::Gate) => {
            "Next: accept the result. loom removes the owner node, its reviewers, its clean worktree and the branch.".to_owned()
        }
        (GateCall::End(MergeEnd::Merged), Receiver::Owner) if target == Target::Coordinator => {
            "Next: nothing. loom set your work to done. If your gate sends rework, commit the fix on this branch and call `loom ready` again.".to_owned()
        }
        (GateCall::End(MergeEnd::Merged), Receiver::Owner) => {
            "Next: nothing. Do not commit to this branch again.".to_owned()
        }
        (GateCall::End(MergeEnd::Failed), Receiver::Root | Receiver::Gate) => format!(
            "Next: decide. After the cause is fixed, the owner calls `loom ready {branch}` again."
        ),
        (GateCall::End(MergeEnd::Failed), Receiver::Owner) => format!(
            "Next: wait for the decision of your gate or root, then fix and call `loom ready {branch}` again."
        ),
    }
}

pub(super) fn withdrawn_step(branch: &str, findings: &str) -> String {
    format!(
        "Next: write your findings so far to {findings}, then stop reviewing {branch}. loom refuses verdicts until the owner's next `loom ready`, which sends you a new round."
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
        "Other reviewers of {branch} and their findings files: {}.",
        listed.join(", ")
    )
}

pub(super) fn review_again_step(
    branch: &str,
    head: &str,
    judged: Option<&str>,
    findings: &str,
) -> String {
    let head = short(head);
    let range = match judged.map(short) {
        None => "the whole branch (you did not judge it before)".to_owned(),
        Some(judged) if judged == head => "only the rebuttals (you judged this head)".to_owned(),
        Some(judged) => format!(
            "only the fixes `git log {judged}..{head}` (the whole branch if {judged} is not an ancestor of {head})"
        ),
    };
    format!(
        "Next: review {range} against your findings file {findings}, the other reviewers' findings and the owner's rebuttals. Write your findings file again, then `loom verdict {branch} {head} pass|block`."
    )
}

pub(super) fn first_round_step(branch: &str, head: &str, target: &str) -> String {
    let head = short(head);
    format!(
        "Next: review `git diff {target}...{head}`, write your findings file, then `loom verdict {branch} {head} pass|block`."
    )
}

const SHORT_HEAD: usize = 12;

fn short(head: &str) -> &str {
    head.get(..SHORT_HEAD).unwrap_or(head)
}
