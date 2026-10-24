use super::*;
use crate::protocol::{Finding, GateState, GateVerb, Judgment, MergeEnd};

fn limits() -> TreeLimits {
    TreeLimits {
        max_depth: None,
        max_children: None,
    }
}

const RENAMED_COORDINATOR: &str = "조율";

fn with_role(g: &Graph, parent: NodeId, name: &str, role: &str) -> NodeId {
    g.add_child(
        parent,
        NewChild {
            role: Some(role),
            worktree: Some("/repo"),
            ..named(name)
        },
        limits(),
        &launched(),
        started,
    )
    .unwrap()
}

fn reviewer(g: &Graph, gate: NodeId, branch: &str, name: &str) -> NodeId {
    g.add_child(
        gate,
        NewChild {
            review: Some(branch),
            role: Some(name),
            ..named(name)
        },
        limits(),
        &launched(),
        started,
    )
    .unwrap()
}

fn via() -> Via {
    Via {
        tool: "loom ready".into(),
        version: Some("0.1.2 (abc)".into()),
    }
}

fn round<'a>(branch: &'a str, target: &'a str) -> Round<'a> {
    Round {
        branch,
        target,
        worktree: "/repo/wt",
        head: "c0ffee1",
    }
}

struct Team {
    lead: NodeId,
    coordinator: NodeId,
    worker: NodeId,
    solo: NodeId,
}

fn team(g: &Graph) -> Team {
    let lead = root(g, "claude");
    let coordinator = with_role(g, lead, "코디-1", RENAMED_COORDINATOR);
    Team {
        lead,
        coordinator,
        worker: with_role(g, coordinator, "worker-x", "워커"),
        solo: with_role(g, lead, "solo", RENAMED_COORDINATOR),
    }
}

fn state(g: &Graph, branch: &str) -> GateState {
    g.lock()
        .query_row(
            "SELECT state FROM gates WHERE branch = ?1",
            [branch],
            |row| from_json(row, 0),
        )
        .unwrap()
}

fn mail(g: &Graph, node: NodeId) -> Vec<String> {
    let taken = g.take(node, Kind::ALL, usize::MAX).unwrap();
    let ids: Vec<MessageId> = taken.iter().map(|d| d.id).collect();
    g.ack(node, &ids).unwrap();
    taken
        .iter()
        .map(|d| {
            let text = crate::profile::message_text(std::slice::from_ref(d));
            text[text.find("(from").unwrap()..].to_owned()
        })
        .collect()
}

fn ended(end: MergeEnd, merged: Option<&str>) -> Ended<'_> {
    Ended {
        end,
        merged,
        cause: None,
        log: Some("/l/batch-1-0.log".into()),
        lines: vec!["batch: 1".into()],
        findings: Vec::new(),
    }
}

fn queued(g: &Graph, owner: NodeId, gate: NodeId, by: NodeId, branch: &str, target: &str) {
    g.ready(owner, &round(branch, target), gate, Some(&via()))
        .unwrap();
    assert_eq!(
        g.verdict(by, branch, Judgment::Pass, None, Some(&via())),
        Ok(GateState::Passed)
    );
}

#[test]
fn spawn_role_is_stored_and_picks_the_gate() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    let role: Option<String> = g
        .lock()
        .query_row("SELECT role FROM nodes WHERE id = ?1", [t.worker.0], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(role.as_deref(), Some("워커"));
    assert_eq!(
        g.gate_for(t.worker).unwrap(),
        GateChoice::Parent {
            gate: t.coordinator,
            worktree: "/repo".into()
        }
    );
    assert_eq!(g.gate_for(t.lead).unwrap(), GateChoice::Owner);
}

#[test]
fn a_coordinator_accepts_a_worker_round_without_reviewers_and_without_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    let v = via();
    let branch = "기능";
    let target = "코디 브랜치";

    assert_eq!(
        g.ready(t.worker, &round(branch, target), t.coordinator, Some(&v)),
        Ok("코디-1".into())
    );
    assert_eq!(state(&g, branch), GateState::Ready);
    let to_gate = mail(&g, t.coordinator);
    assert_eq!(to_gate.len(), 1);
    assert!(
        to_gate[0].starts_with(
            "(from: worker-x, kind: ready)\nbranch: 기능\nhead: c0ffee1\nworktree: /repo/wt\ntarget: 코디 브랜치\nNext: accept or return the branch yourself"
        ),
        "{}",
        to_gate[0]
    );
    assert!(matches!(
        g.ready(t.worker, &round(branch, target), t.coordinator, Some(&v)),
        Err(Refusal::Gate(e)) if e.state == GateState::Ready && e.allowed == [GateVerb::Verdict, GateVerb::Fail]
    ));

    let accepts = Refusal::CoordinatorAccepts {
        branch: branch.into(),
        gate: "코디-1".into(),
    };
    let spawned = g
        .add_child(
            t.coordinator,
            NewChild {
                review: Some(branch),
                ..named("reviewer")
            },
            limits(),
            &launched(),
            started,
        )
        .map(|_| ());
    assert_eq!(spawned, Err(accepts.clone()));
    let plain = g
        .add_child(
            t.coordinator,
            named("plain"),
            limits(),
            &launched(),
            started,
        )
        .unwrap();
    for other in [plain, t.solo] {
        assert_eq!(
            g.verdict(other, branch, Judgment::Pass, None, Some(&v)),
            Err(accepts.clone())
        );
    }
    assert_eq!(
        g.verdict(t.worker, branch, Judgment::Pass, None, Some(&v)),
        Err(Refusal::OwnVerdict(branch.into()))
    );
    assert_eq!(
        g.verdict(t.coordinator, branch, Judgment::Block, None, Some(&v)),
        Err(Refusal::NoFindings)
    );
    assert_eq!(
        g.verdict(
            t.coordinator,
            branch,
            Judgment::Block,
            Some("/r/findings.md"),
            Some(&v)
        ),
        Ok(GateState::Working)
    );
    let rework = mail(&g, t.worker);
    assert_eq!(rework.len(), 1);
    assert!(
        rework[0].starts_with(
            "(from: 코디-1, kind: verdict)\nbranch: 기능\nhead: c0ffee1\njudgment: block\nfindings: /r/findings.md\n"
        ) && rework[0].contains("\nNext: fix each finding"),
        "{}",
        rework[0]
    );
    let reviewer_name: Option<String> = g
        .lock()
        .query_row("SELECT verdict ->> '$.reviewer.name' FROM gates", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(reviewer_name.as_deref(), Some("코디-1"));

    queued(&g, t.worker, t.coordinator, t.coordinator, branch, target);
    assert!(
        mail(&g, t.worker)[0]
            .contains("kind: verdict)\nbranch: 기능\nhead: c0ffee1\njudgment: pass")
    );
    mail(&g, t.coordinator);
    let repo = RepoId(1);
    assert_eq!(g.queued_targets().unwrap(), [(repo, target.to_owned())]);
    let queue = g.queue(repo, target).unwrap();
    assert_eq!(
        queue
            .iter()
            .map(|q| (q.branch.as_str(), q.owner, q.head.as_str()))
            .collect::<Vec<_>>(),
        [(branch, t.worker, "c0ffee1")]
    );
    let id = g.batch_open(repo, target, &[branch]).unwrap();
    assert_eq!(state(&g, branch), GateState::Merging);
    assert!(g.queue(repo, target).unwrap().is_empty());
    assert!(mail(&g, t.worker).is_empty() && mail(&g, t.coordinator).is_empty());
    assert!(matches!(
        g.verdict(t.coordinator, branch, Judgment::Block, Some("/r/late.md"), Some(&v)),
        Err(Refusal::Gate(e)) if e.state == GateState::Merging && e.allowed.is_empty()
    ));
    assert_eq!(
        g.merge_end(repo, branch, ended(MergeEnd::Merged, Some("abc1234"))),
        Ok(GateState::Merged)
    );
    g.batch_close(id).unwrap();
    let merged = mail(&g, t.worker);
    assert_eq!(merged.len(), 1);
    assert!(
        merged[0].starts_with(
            "(from: loom, kind: merged)\nbranch: 기능\nhead: c0ffee1\nsha: abc1234\nlog: /l/batch-1-0.log\nbatch: 1\nNext:"
        ),
        "{}",
        merged[0]
    );
    assert_eq!(
        mail(&g, t.coordinator).len(),
        1,
        "the gate hears the merge too"
    );
    assert!(mail(&g, t.lead).is_empty());
    assert_eq!(g.gate_merges(repo, target).unwrap(), ["abc1234"]);
    g.ready(t.worker, &round(branch, target), t.coordinator, Some(&v))
        .unwrap();
    assert_eq!(state(&g, branch), GateState::Ready);
    assert_eq!(g.gate_merges(repo, target).unwrap(), ["abc1234"]);
    assert!(g.gate_merges(repo, "main").unwrap().is_empty());
}

#[test]
fn a_coordinator_branch_into_main_still_needs_its_own_reviewer() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    let repo = RepoId(1);
    g.ready(
        t.coordinator,
        &round("코디", "main"),
        t.coordinator,
        Some(&via()),
    )
    .unwrap();
    assert_eq!(state(&g, "코디"), GateState::Ready);
    assert_eq!(
        g.verdict(t.coordinator, "코디", Judgment::Pass, None, Some(&via())),
        Err(Refusal::OwnVerdict("코디".into()))
    );
    assert!(g.queue(repo, "main").unwrap().is_empty());
    let r = reviewer(&g, t.coordinator, "코디", "r");
    assert_eq!(
        g.verdict(r, "코디", Judgment::Pass, None, Some(&via())),
        Ok(GateState::Passed)
    );
    assert_eq!(g.queue(repo, "main").unwrap().len(), 1);
}

#[test]
fn an_owner_gated_round_tells_the_root_about_a_failure() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    assert_eq!(
        g.ready(t.solo, &round("b", "main"), t.solo, Some(&via())),
        Ok("solo".into())
    );
    let r = reviewer(&g, t.solo, "b", "solo-reviewer");
    assert_eq!(mail(&g, t.solo).len(), 1);
    g.verdict(r, "b", Judgment::Pass, None, Some(&via()))
        .unwrap();
    mail(&g, t.solo);
    let id = g.batch_open(RepoId(1), "main", &["b"]).unwrap();
    let failed = Ended {
        cause: Some("publishing the batch exited 1".into()),
        ..ended(MergeEnd::Failed, None)
    };
    assert_eq!(g.merge_end(RepoId(1), "b", failed), Ok(GateState::Failed));
    g.batch_close(id).unwrap();
    let to_root = mail(&g, t.lead);
    assert_eq!(to_root.len(), 1);
    assert!(
        to_root[0].starts_with(
            "(from: loom, kind: failed)\nbranch: b\nhead: c0ffee1\ncause: publishing the batch exited 1\n"
        ),
        "{}",
        to_root[0]
    );
    assert_eq!(mail(&g, t.solo).len(), 1, "the owner hears the failure too");
}

#[test]
fn a_withdrawn_round_tells_exactly_its_reviewers_to_stop() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    let own_task = g
        .add_child(t.solo, named("own-task"), limits(), &launched(), started)
        .unwrap();
    g.ready(t.solo, &round("b", "main"), t.solo, Some(&via()))
        .unwrap();
    g.ready(t.solo, &round("x", "main"), t.solo, Some(&via()))
        .unwrap();
    let first = reviewer(&g, t.solo, "b", "first");
    let second = reviewer(&g, t.solo, "b", "second");
    let gone = reviewer(&g, t.solo, "b", "gone");
    let other = reviewer(&g, t.solo, "x", "other");
    g.exited(gone, 0).unwrap();
    mail(&g, t.solo);
    assert_eq!(
        g.verdict(first, "b", Judgment::Block, Some("/r/f.md"), Some(&via())),
        Ok(GateState::Working)
    );
    let got = mail(&g, second);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(
        got[0].starts_with(
            "(from: loom, kind: withdrawn)\nbranch: b\nhead: c0ffee1\nNext: stop reviewing b."
        ),
        "{}",
        got[0]
    );
    for node in [first, gone, other, own_task, t.lead] {
        assert!(mail(&g, node).is_empty(), "{node}");
    }
    let traced = g.recent_events().unwrap().into_iter().any(|r| {
        matches!(
            &r.entry,
            Entry::Message { what: Some(MessageKind::Withdrawn { branch, .. }), gate: Some(_), .. } if branch == "b"
        )
    });
    assert!(traced, "withdrawn is traced");
    g.ready(t.solo, &round("b", "main"), t.solo, Some(&via()))
        .unwrap();
    for node in [first, second] {
        let got = mail(&g, node);
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(
            got[0].starts_with("(from: solo, kind: ready)\nbranch: b\n"),
            "{}",
            got[0]
        );
        assert!(
            got[0].contains("Next: a new round of b started."),
            "{}",
            got[0]
        );
    }
    for node in [gone, other, own_task] {
        assert!(mail(&g, node).is_empty(), "{node}");
    }
    let third = reviewer(&g, t.solo, "b", "third");
    mail(&g, t.solo);
    assert_eq!(
        g.fail(t.solo, "b", "not needed", Some(&via())),
        Ok(GateState::Failed)
    );
    for node in [third, first, second] {
        assert_eq!(mail(&g, node).len(), 1, "{node}");
    }
    assert!(mail(&g, other).is_empty());
}

#[test]
fn a_pass_waits_for_every_live_reviewer_of_the_branch() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    let repo = RepoId(1);
    g.ready(t.solo, &round("b", "main"), t.solo, Some(&via()))
        .unwrap();
    let first = reviewer(&g, t.solo, "b", "first");
    let second = reviewer(&g, t.solo, "b", "second");
    let quiet = reviewer(&g, t.solo, "b", "quiet");
    assert_eq!(
        g.verdict(first, "b", Judgment::Pass, None, Some(&via())),
        Ok(GateState::Passed)
    );
    assert!(
        g.queued_targets().unwrap().is_empty(),
        "second and quiet have not judged"
    );
    g.exited(quiet, 0).unwrap();
    assert!(
        g.queue(repo, "main").unwrap().is_empty(),
        "second has not judged"
    );
    g.ready(t.solo, &round("x", "main"), t.solo, Some(&via()))
        .unwrap();
    let other = reviewer(&g, t.solo, "x", "other");
    g.verdict(other, "x", Judgment::Pass, None, Some(&via()))
        .unwrap();
    let names = |q: Vec<Queued>| q.into_iter().map(|q| q.branch).collect::<Vec<_>>();
    assert_eq!(names(g.queue(repo, "main").unwrap()), ["x"]);
    for node in [t.solo, first, other] {
        mail(&g, node);
    }
    assert_eq!(
        g.verdict(second, "b", Judgment::Block, Some("/r/f.md"), Some(&via())),
        Ok(GateState::Working)
    );
    assert_eq!(mail(&g, first).len(), 1, "first hears withdrawn");
    g.ready(t.solo, &round("b", "main"), t.solo, Some(&via()))
        .unwrap();
    assert_eq!(
        g.verdict(second, "b", Judgment::Pass, None, Some(&via())),
        Ok(GateState::Passed)
    );
    assert_eq!(
        names(g.queue(repo, "main").unwrap()),
        ["x"],
        "first has not judged round 2"
    );
    g.verdict(first, "b", Judgment::Pass, None, Some(&via()))
        .unwrap();
    assert_eq!(names(g.queue(repo, "main").unwrap()), ["x", "b"]);
}

#[test]
fn a_reviewer_of_another_branch_never_holds_this_branch() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    g.ready(t.solo, &round("b", "main"), t.solo, Some(&via()))
        .unwrap();
    g.ready(t.solo, &round("x", "main"), t.solo, Some(&via()))
        .unwrap();
    let first = reviewer(&g, t.solo, "b", "first");
    reviewer(&g, t.solo, "x", "other");
    mail(&g, t.solo);
    g.verdict(first, "b", Judgment::Pass, None, Some(&via()))
        .unwrap();
    assert_eq!(g.queue(RepoId(1), "main").unwrap().len(), 1);
    assert!(mail(&g, t.solo)[0].contains("Next: nothing. loom queued the branch"));
}

#[test]
fn a_held_pass_names_the_reviewers_that_hold_the_branch() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    g.ready(t.solo, &round("b", "main"), t.solo, Some(&via()))
        .unwrap();
    let first = reviewer(&g, t.solo, "b", "first");
    let second = reviewer(&g, t.solo, "b", "second");
    g.verdict(first, "b", Judgment::Pass, None, Some(&via()))
        .unwrap();
    g.verdict(second, "b", Judgment::Block, Some("/r/f.md"), Some(&via()))
        .unwrap();
    g.ready(t.solo, &round("b", "main"), t.solo, Some(&via()))
        .unwrap();
    mail(&g, t.solo);
    g.verdict(second, "b", Judgment::Pass, None, Some(&via()))
        .unwrap();
    let held = mail(&g, t.solo);
    assert_eq!(held.len(), 1);
    assert!(
        held[0].contains("loom holds the branch until first judge it")
            && !held[0].contains("loom queued"),
        "{}",
        held[0]
    );
    g.verdict(first, "b", Judgment::Pass, None, Some(&via()))
        .unwrap();
    let queued = mail(&g, t.solo);
    assert!(
        queued[0].contains("Next: nothing. loom queued the branch"),
        "{}",
        queued[0]
    );
}

#[test]
fn a_reviewer_withdrawn_before_its_verdict_cannot_judge_another_branch_and_still_holds() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    let repo = RepoId(1);
    g.ready(t.solo, &round("b", "main"), t.solo, Some(&via()))
        .unwrap();
    let a = reviewer(&g, t.solo, "b", "a");
    let bee = reviewer(&g, t.solo, "b", "bee");
    g.verdict(a, "b", Judgment::Block, Some("/r/f.md"), Some(&via()))
        .unwrap();
    assert_eq!(mail(&g, bee).len(), 1, "bee hears withdrawn");
    g.ready(t.solo, &round("x", "main"), t.solo, Some(&via()))
        .unwrap();
    assert_eq!(
        g.verdict(bee, "x", Judgment::Pass, None, Some(&via())),
        Err(Refusal::NotReviewer {
            branch: "x".into(),
            review: Some("b".into())
        })
    );
    g.ready(t.solo, &round("b", "main"), t.solo, Some(&via()))
        .unwrap();
    mail(&g, t.solo);
    g.verdict(a, "b", Judgment::Pass, None, Some(&via()))
        .unwrap();
    assert!(g.queue(repo, "main").unwrap().is_empty());
    assert!(
        mail(&g, t.solo)[0].contains("until bee judge it"),
        "the pass names bee"
    );
    g.verdict(bee, "b", Judgment::Pass, None, Some(&via()))
        .unwrap();
    assert_eq!(g.queue(repo, "main").unwrap().len(), 1);
}

#[test]
fn review_binds_a_child_to_a_branch_of_its_gate() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    let spawn = |parent: NodeId, branch: &str, name: &str| {
        g.add_child(
            parent,
            NewChild {
                review: Some(branch),
                ..named(name)
            },
            limits(),
            &launched(),
            started,
        )
    };
    assert_eq!(
        spawn(t.solo, "nope", "r0"),
        Err(Refusal::NoRound("nope".into()))
    );
    g.ready(t.solo, &round("w", "main"), t.solo, Some(&via()))
        .unwrap();
    assert!(matches!(
        spawn(t.coordinator, "w", "r1"),
        Err(Refusal::NotGate { gate, .. }) if gate == "solo"
    ));
    let plain = g
        .add_child(t.solo, named("plain"), limits(), &launched(), started)
        .unwrap();
    assert_eq!(
        g.verdict(plain, "w", Judgment::Pass, None, Some(&via())),
        Err(Refusal::NotReviewer {
            branch: "w".into(),
            review: None
        })
    );
    let r = spawn(t.solo, "w", "r2").unwrap();
    assert_eq!(
        g.verdict(r, "w", Judgment::Pass, None, Some(&via())),
        Ok(GateState::Passed)
    );
}

#[test]
fn a_reused_reviewer_that_exits_stops_holding_the_branch() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    let repo = RepoId(1);
    g.ready(t.solo, &round("b", "main"), t.solo, Some(&via()))
        .unwrap();
    let first = reviewer(&g, t.solo, "b", "first");
    let second = reviewer(&g, t.solo, "b", "second");
    g.verdict(first, "b", Judgment::Pass, None, Some(&via()))
        .unwrap();
    g.verdict(second, "b", Judgment::Block, Some("/r/f.md"), Some(&via()))
        .unwrap();
    g.ready(t.solo, &round("b", "main"), t.solo, Some(&via()))
        .unwrap();
    g.verdict(second, "b", Judgment::Pass, None, Some(&via()))
        .unwrap();
    assert!(g.queue(repo, "main").unwrap().is_empty());
    g.exited(first, 0).unwrap();
    assert_eq!(g.queue(repo, "main").unwrap().len(), 1);
}

#[test]
fn an_old_ci_round_tells_the_root_because_its_target_is_the_base() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    let ci = g
        .add_child(t.lead, named("ci"), limits(), &launched(), started)
        .unwrap();
    g.ready(t.solo, &round("b", "main"), t.solo, Some(&via()))
        .unwrap();
    g.lock()
        .execute(
            r#"UPDATE gates SET gate = ?1, state = '"merging"', batch = NULL"#,
            [ci.0],
        )
        .unwrap();
    for node in [t.lead, t.solo, ci] {
        mail(&g, node);
    }
    assert_eq!(g.orphans().unwrap(), [(RepoId(1), "b".to_owned())]);
    let failed = Ended {
        cause: Some("loom upgrade: send loom ready again".into()),
        ..ended(MergeEnd::Failed, None)
    };
    assert_eq!(g.merge_end(RepoId(1), "b", failed), Ok(GateState::Failed));
    for node in [t.lead, t.solo, ci] {
        assert_eq!(mail(&g, node).len(), 1, "{node}");
    }
}

#[test]
fn passes_in_one_second_keep_the_pass_order() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    for branch in ["a", "b"] {
        g.ready(t.solo, &round(branch, "main"), t.solo, Some(&via()))
            .unwrap();
    }
    for branch in ["b", "a"] {
        let r = reviewer(&g, t.solo, branch, &format!("r-{branch}"));
        g.verdict(r, branch, Judgment::Pass, None, Some(&via()))
            .unwrap();
    }
    g.lock().execute("UPDATE gates SET queued = 1", []).unwrap();
    let names: Vec<String> = g
        .queue(RepoId(1), "main")
        .unwrap()
        .into_iter()
        .map(|q| q.branch)
        .collect();
    assert_eq!(names, ["b", "a"]);
}

#[test]
fn a_stale_batch_keeps_its_members_and_requeue_keeps_the_queue_order() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    let repo = RepoId(1);
    for branch in ["a", "b"] {
        g.ready(t.solo, &round(branch, "main"), t.solo, Some(&via()))
            .unwrap();
        let r = reviewer(&g, t.solo, branch, &format!("r-{branch}"));
        g.verdict(r, branch, Judgment::Pass, None, Some(&via()))
            .unwrap();
    }
    let id = g.batch_open(repo, "main", &["a", "b"]).unwrap();
    g.batch_checked(id, "base0", "tip0").unwrap();
    let stale = g.stale_batches().unwrap();
    assert_eq!(stale.len(), 1);
    assert_eq!(
        (
            stale[0].id,
            stale[0].base.as_deref(),
            stale[0].tip.as_deref()
        ),
        (id, Some("base0"), Some("tip0"))
    );
    let names = |members: &[Queued]| members.iter().map(|m| m.branch.clone()).collect::<Vec<_>>();
    assert_eq!(names(&stale[0].members), ["a", "b"]);
    assert_eq!(g.requeue(repo, "b"), Ok(GateState::Passed));
    assert_eq!(g.requeue(repo, "a"), Ok(GateState::Passed));
    assert_eq!(names(&g.queue(repo, "main").unwrap()), ["a", "b"]);
    assert!(g.stale_batches().unwrap()[0].members.is_empty());
    let id2 = g.batch_open(repo, "main", &["a"]).unwrap();
    g.batch_close(id2).unwrap();
    assert_eq!(g.stale_batches().unwrap().len(), 2);
    g.requeue(repo, "a").unwrap();
    g.batch_close(id2).unwrap();
    g.batch_close(id).unwrap();
    assert!(g.stale_batches().unwrap().is_empty());
    assert!(matches!(g.requeue(repo, "a"), Err(Refusal::Gate(_))));
}

#[test]
fn a_queue_result_without_receivers_is_still_traced() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    g.ready(
        t.worker,
        &round("b", "코디 브랜치"),
        t.coordinator,
        Some(&via()),
    )
    .unwrap();
    g.verdict(t.coordinator, "b", Judgment::Pass, None, Some(&via()))
        .unwrap();
    g.batch_open(RepoId(1), "코디 브랜치", &["b"]).unwrap();
    g.remove(t.worker).unwrap();
    mail(&g, t.coordinator);
    assert_eq!(
        g.merge_end(RepoId(1), "b", ended(MergeEnd::Merged, Some("abc1234"))),
        Ok(GateState::Merged)
    );
    assert_eq!(mail(&g, t.coordinator).len(), 1);
    let recent = g.recent_events().unwrap();
    assert!(recent.iter().any(|r| matches!(
        &r.entry,
        Entry::Message { gate: Some(trace), .. } if trace.to == Some(GateState::Merged)
    )));
}

#[test]
fn fail_and_no_round_and_unknown_callers_are_refused_or_routed() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    let v = via();
    assert_eq!(
        g.verdict(t.coordinator, "없음", Judgment::Pass, None, Some(&v)),
        Err(Refusal::NoRound("없음".into()))
    );
    assert_eq!(
        g.fail(t.coordinator, "없음", "x", Some(&v)),
        Err(Refusal::NoRound("없음".into()))
    );
    g.ready(
        t.worker,
        &round("b", "코디 브랜치"),
        t.coordinator,
        Some(&v),
    )
    .unwrap();
    mail(&g, t.coordinator);
    assert!(matches!(
        g.fail(t.worker, "b", "x", Some(&v)),
        Err(Refusal::NotGate { .. })
    ));
    assert!(matches!(
        g.requeue(RepoId(1), "b"),
        Err(Refusal::Gate(e)) if e.state == GateState::Ready
    ));
    assert_eq!(
        g.fail(t.coordinator, "b", "not needed", Some(&v)),
        Ok(GateState::Failed)
    );
    let to_owner = mail(&g, t.worker);
    assert!(
        to_owner[0].starts_with(
            "(from: 코디-1, kind: failed)\nbranch: b\nhead: c0ffee1\ncause: not needed\nNext:"
        ),
        "{}",
        to_owner[0]
    );
    assert!(
        mail(&g, t.lead).is_empty(),
        "no root copy for a coordinator target"
    );
    assert_eq!(
        g.ready(
            t.worker,
            &round("b", "코디 브랜치"),
            t.coordinator,
            Some(&v)
        ),
        Ok("코디-1".into())
    );
}

#[test]
fn every_gate_call_and_refusal_leaves_a_trace() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    g.set_model(
        t.coordinator,
        &Model {
            model: Some("claude-opus-5-5".into()),
            effort: None,
        },
    )
    .unwrap();
    let v = via();
    g.ready(t.coordinator, &round("b", "main"), t.coordinator, Some(&v))
        .unwrap();
    let refusal = Refusal::Uncommitted("b".into());
    g.record_gate_refusal(t.coordinator, GateVerb::Ready, "b", &refusal, Some(&v))
        .unwrap();
    let recent = g.recent_events().unwrap();
    let Entry::Refused {
        reason,
        via,
        gate: Some(trace),
        ..
    } = &recent[0].entry
    else {
        panic!("{:?}", recent[0].entry);
    };
    assert_eq!((reason, via.as_ref()), (&refusal, Some(&v)));
    assert_eq!((trace.from, trace.to), (Some(GateState::Ready), None));
    let Entry::Message {
        via,
        gate: Some(trace),
        ..
    } = &recent[1].entry
    else {
        panic!("{:?}", recent[1].entry);
    };
    assert_eq!(via.as_ref(), Some(&v));
    assert_eq!(trace.verb, GateVerb::Ready);
    assert_eq!(trace.branch, "b");
    assert_eq!(
        (trace.from, trace.to),
        (Some(GateState::Working), Some(GateState::Ready))
    );
    assert_eq!(trace.role.as_deref(), Some(RENAMED_COORDINATOR));
    assert_eq!(trace.agent, None, "a shell node");
    assert_eq!(trace.model.as_deref(), Some("claude-opus-5-5"));
    let send_via = Via {
        tool: "loom send".into(),
        version: None,
    };
    g.note(t.worker, "코디-1", Kind::Queue, "hi", Some(&send_via))
        .unwrap();
    let recent = g.recent_events().unwrap();
    assert!(
        matches!(&recent[0].entry, Entry::Message { via: Some(got), gate: None, .. } if *got == send_via)
    );
}

#[test]
fn old_event_rows_without_via_or_gate_still_read() {
    let old = r#"{"kind":"message","how":"queue","from":{"id":1,"name":"a"},"to":{"id":2,"name":"b"},"step":"sent"}"#;
    let entry: Entry = serde_json::from_str(old).unwrap();
    assert!(matches!(
        entry,
        Entry::Message {
            via: None,
            gate: None,
            ..
        }
    ));
    let old = r#"{"kind":"refused","how":"queue","from":{"id":1,"name":"a"},"to":"b","reason":{"code":"emptyName"}}"#;
    let entry: Entry = serde_json::from_str(old).unwrap();
    assert!(matches!(
        entry,
        Entry::Refused {
            via: None,
            gate: None,
            ..
        }
    ));
}

#[test]
fn audit_finds_raw_gate_words_of_the_owner_subtree_in_this_round_and_a_pruned_range() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    let stranger = root(&g, "codex");
    let v = via();
    let before = g
        .note(t.worker, "코디-1", Kind::Queue, "READY old", None)
        .unwrap();
    g.lock()
        .execute(
            "UPDATE events SET at = at - 10 WHERE message = ?1",
            [before.id.0],
        )
        .unwrap();
    g.ready(
        t.worker,
        &round("b", "코디 브랜치"),
        t.coordinator,
        Some(&v),
    )
    .unwrap();
    g.lock()
        .execute("UPDATE gates SET opened = opened - 5", [])
        .unwrap();
    let raw = g
        .note(t.worker, "코디-1", Kind::Queue, "READY b /x", None)
        .unwrap();
    g.note(stranger, "코디-1", Kind::Queue, "READY other", None)
        .unwrap();
    g.note(t.lead, "코디-1", Kind::Queue, "MERGED other", None)
        .unwrap();
    g.note(t.worker, "코디-1", Kind::Queue, "Ready for review", None)
        .unwrap();
    g.human_typed(t.worker, "READY now").unwrap();
    let since: i64 = g
        .lock()
        .query_row("SELECT opened FROM gates WHERE branch = 'b'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        g.audit(t.worker, since).unwrap(),
        vec![Finding::RawGateWord {
            event: g
                .lock()
                .query_row(
                    "SELECT id FROM events WHERE message = ?1",
                    [raw.id.0],
                    |r| r.get(0)
                )
                .unwrap(),
            from: "worker-x".into(),
            word: "READY".into(),
        }]
    );
    let oldest: i64 = g
        .lock()
        .query_row("SELECT min(at) FROM events", [], |r| r.get(0))
        .unwrap();
    let findings = g.audit(t.worker, oldest - 1).unwrap();
    assert_eq!(
        findings[0],
        Finding::RangeIncomplete {
            since: oldest - 1,
            oldest
        }
    );
}

#[test]
fn an_ungated_ready_reaches_the_gate_without_a_gate_row() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    let v = via();
    assert!(!g.ready_sent(t.worker, "b", "c0ffee1").unwrap());
    assert_eq!(
        g.announce_ready(t.worker, &round("b", "main"), t.coordinator, Some(&v)),
        Ok("코디-1".into())
    );
    let to_ci = mail(&g, t.coordinator);
    assert_eq!(to_ci.len(), 1);
    assert!(
        to_ci[0].starts_with(
            "(from: worker-x, kind: ready)\nbranch: b\nhead: c0ffee1\nworktree: /repo/wt\ntarget: main\nNext: review the branch, merge it into main by hand"
        ),
        "{}",
        to_ci[0]
    );
    let rows: u32 = g
        .lock()
        .query_row("SELECT count(*) FROM gates", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0, "no gate state machine here");
    assert!(g.ready_sent(t.worker, "b", "c0ffee1").unwrap());
    assert!(!g.ready_sent(t.worker, "b", "other").unwrap());
}

fn spawned(
    g: &Graph,
    parent: NodeId,
    name: &str,
    role: &str,
    review: Option<&str>,
) -> Result<NodeId, Refusal> {
    g.add_child(
        parent,
        NewChild {
            role: Some(role),
            review,
            ..named(name)
        },
        limits(),
        &launched(),
        started,
    )
}

#[test]
fn the_lead_spawns_only_worktree_owners_and_the_reviewers_of_its_own_branch() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    for role in ["코디", "워커", "검토자", RENAMED_COORDINATOR] {
        assert_eq!(
            spawned(&g, t.lead, "w", role, None),
            Err(Refusal::LeadChildNeedsWorktree(role.into()))
        );
    }
    for role in ["코디", "워커", "아무 역할"] {
        with_role(&g, t.lead, &format!("{role}-2"), role);
    }
    assert!(spawned(&g, t.coordinator, "w", "워커", None).is_ok());
    assert!(
        g.add_child(t.lead, named("shell"), limits(), &launched(), started)
            .is_ok()
    );
    g.ready(t.lead, &round("lead-fix", "main"), t.lead, Some(&via()))
        .unwrap();
    assert!(spawned(&g, t.lead, "검토자 lead-fix", "검토자", Some("lead-fix")).is_ok());
}

#[test]
fn a_branch_has_one_reviewer_per_role_even_after_its_process_ends() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    g.ready(t.solo, &round("b", "main"), t.solo, Some(&via()))
        .unwrap();
    g.ready(t.solo, &round("x", "main"), t.solo, Some(&via()))
        .unwrap();
    let first = spawned(&g, t.solo, "검토자 b", "검토자", Some("b")).unwrap();
    let seated = Err(Refusal::ReviewerSeated {
        branch: "b".into(),
        role: "검토자".into(),
        reviewer: "검토자 b".into(),
    });
    assert_eq!(spawned(&g, t.solo, "검토자 b", "검토자", Some("b")), seated);
    assert!(spawned(&g, t.solo, "보조 검토자 b", "보조 검토자", Some("b")).is_ok());
    assert!(spawned(&g, t.solo, "검토자 x", "검토자", Some("x")).is_ok());
    g.exited(first, 0).unwrap();
    assert_eq!(
        spawned(&g, t.solo, "검토자3 b", "검토자", Some("b")),
        seated
    );
}
