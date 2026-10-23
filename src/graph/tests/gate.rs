use super::*;
use crate::protocol::{COORDINATOR_ROLE, Finding, GateState, GateVerb, Judgment, MergeEnd};

fn limits() -> TreeLimits {
    TreeLimits {
        max_depth: None,
        max_children: None,
    }
}

fn with_role(g: &Graph, parent: NodeId, name: &str, role: &str) -> NodeId {
    g.add_child(
        parent,
        NewChild {
            role: Some(role),
            ..named(name)
        },
        limits(),
        &launched(),
        started,
    )
    .unwrap()
}

fn reviewer(g: &Graph, gate: NodeId, gate_name: &str, name: &str) -> NodeId {
    g.add_child(
        gate,
        NewChild {
            task: gate_name,
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
    let coordinator = with_role(g, lead, "코디-1", COORDINATOR_ROLE);
    Team {
        lead,
        coordinator,
        worker: with_role(g, coordinator, "worker-x", "워커"),
        solo: with_role(g, lead, "solo", "워커"),
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
        GateChoice::Coordinator {
            gate: t.coordinator,
            worktree: "/repo".into()
        }
    );
    assert_eq!(g.gate_for(t.coordinator).unwrap(), GateChoice::Owner);
    assert_eq!(g.gate_for(t.solo).unwrap(), GateChoice::Owner);
}

#[test]
fn a_coordinator_round_goes_ready_block_ready_pass_queue_merge_without_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    let r = reviewer(&g, t.coordinator, "코디-1", "reviewer");
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
            "(from: worker-x, kind: ready)\nbranch: 기능\nhead: c0ffee1\nworktree: /repo/wt\ntarget: 코디 브랜치\nNext: spawn a reviewer"
        ),
        "{}",
        to_gate[0]
    );
    assert!(matches!(
        g.ready(t.worker, &round(branch, target), t.coordinator, Some(&v)),
        Err(Refusal::Gate(e)) if e.state == GateState::Ready && e.allowed == [GateVerb::Verdict, GateVerb::Fail]
    ));

    for other in [t.coordinator, t.solo] {
        assert!(matches!(
            g.verdict(other, branch, Judgment::Pass, None, Some(&v)),
            Err(Refusal::NotGate { gate, .. }) if gate == "코디-1"
        ));
    }
    assert_eq!(
        g.verdict(t.worker, branch, Judgment::Pass, None, Some(&v)),
        Err(Refusal::OwnVerdict(branch.into()))
    );
    assert_eq!(
        g.verdict(r, branch, Judgment::Block, None, Some(&v)),
        Err(Refusal::NoFindings)
    );
    assert_eq!(
        g.verdict(r, branch, Judgment::Block, Some("/r/findings.md"), Some(&v)),
        Ok(GateState::Working)
    );
    let rework = mail(&g, t.worker);
    assert_eq!(rework.len(), 1);
    assert!(
        rework[0].starts_with(
            "(from: reviewer, kind: verdict)\nbranch: 기능\nhead: c0ffee1\njudgment: block\nfindings: /r/findings.md\n"
        ) && rework[0].contains("\nNext: fix each finding"),
        "{}",
        rework[0]
    );
    assert_eq!(
        mail(&g, t.coordinator).len(),
        1,
        "the gate hears its reviewer's block"
    );
    let reviewer_name: Option<String> = g
        .lock()
        .query_row("SELECT verdict ->> '$.reviewer.name' FROM gates", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(reviewer_name.as_deref(), Some("reviewer"));

    let second = reviewer(&g, t.coordinator, "코디-1", "reviewer-2");
    queued(&g, t.worker, t.coordinator, r, branch, target);
    assert!(
        mail(&g, t.worker)[0]
            .contains("kind: verdict)\nbranch: 기능\nhead: c0ffee1\njudgment: pass")
    );
    assert_eq!(
        g.verdict(second, branch, Judgment::Pass, None, Some(&v)),
        Ok(GateState::Passed)
    );
    mail(&g, t.worker);
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
        g.verdict(r, branch, Judgment::Block, Some("/r/late.md"), Some(&v)),
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
fn an_owner_gated_round_tells_the_root_about_a_failure() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    let r = reviewer(&g, t.solo, "solo", "solo-reviewer");
    assert_eq!(
        g.ready(t.solo, &round("b", "main"), t.solo, Some(&via())),
        Ok("solo".into())
    );
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
    let earlier = reviewer(&g, t.solo, "solo", "earlier");
    let own_task = g
        .add_child(t.solo, named("own-task"), limits(), &launched(), started)
        .unwrap();
    g.ready(t.solo, &round("b", "main"), t.solo, Some(&via()))
        .unwrap();
    let first = reviewer(&g, t.solo, "solo", "first");
    let second = reviewer(&g, t.solo, "solo", "second");
    let gone = reviewer(&g, t.solo, "solo", "gone");
    g.exited(gone, 0).unwrap();
    mail(&g, t.solo);
    assert_eq!(
        g.verdict(earlier, "b", Judgment::Pass, None, Some(&via())),
        Ok(GateState::Passed)
    );
    assert_eq!(
        g.verdict(first, "b", Judgment::Block, Some("/r/f.md"), Some(&via())),
        Ok(GateState::Working)
    );
    for node in [earlier, second] {
        let got = mail(&g, node);
        assert_eq!(got.len(), 1, "{node}: {got:?}");
        assert!(
            got[0].starts_with(
                "(from: loom, kind: withdrawn)\nbranch: b\nhead: c0ffee1\nNext: stop reviewing b."
            ),
            "{}",
            got[0]
        );
    }
    for node in [first, gone, own_task, t.lead] {
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
    let third = reviewer(&g, t.solo, "solo", "third");
    mail(&g, t.solo);
    assert_eq!(
        g.fail(t.solo, "b", "not needed", Some(&via())),
        Ok(GateState::Failed)
    );
    assert_eq!(mail(&g, third).len(), 1);
    for node in [earlier, first, second] {
        assert!(mail(&g, node).is_empty(), "{node}");
    }
}

#[test]
fn a_pass_waits_for_every_live_reviewer_of_the_round() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    let repo = RepoId(1);
    let first = reviewer(&g, t.solo, "solo", "first");
    g.ready(t.solo, &round("b", "main"), t.solo, Some(&via()))
        .unwrap();
    let second = reviewer(&g, t.solo, "solo", "second");
    let quiet = reviewer(&g, t.solo, "solo", "quiet");
    assert_eq!(
        g.verdict(first, "b", Judgment::Pass, None, Some(&via())),
        Ok(GateState::Passed)
    );
    assert!(
        g.queued_targets().unwrap().is_empty(),
        "second and quiet have not judged"
    );
    assert!(g.queue(repo, "main").unwrap().is_empty());
    g.exited(quiet, 0).unwrap();
    assert!(
        g.queue(repo, "main").unwrap().is_empty(),
        "second has not judged"
    );
    g.ready(t.solo, &round("x", "main"), t.solo, Some(&via()))
        .unwrap();
    let other = reviewer(&g, t.solo, "solo", "other");
    g.verdict(other, "x", Judgment::Pass, None, Some(&via()))
        .unwrap();
    let names = |q: Vec<Queued>| q.into_iter().map(|q| q.branch).collect::<Vec<_>>();
    assert_eq!(names(g.queue(repo, "main").unwrap()), ["x"]);
    g.lock()
        .execute(
            r#"INSERT INTO events (at, entry) VALUES (unixepoch(), '{"kind":"message","how":"queue","from":{"id":999,"name":"far"},"to":{"id":998,"name":"g"},"step":"sent","gate":{"verb":"ready","branch":"b","from":"working","to":"ready","role":null,"agent":null,"model":null,"findings":[]}}')"#,
            [],
        )
        .unwrap();
    for node in [t.solo, first, other] {
        mail(&g, node);
    }
    assert_eq!(
        g.verdict(second, "b", Judgment::Block, Some("/r/f.md"), Some(&via())),
        Ok(GateState::Working)
    );
    assert_eq!(
        mail(&g, first).len(),
        1,
        "first judged this round and hears withdrawn"
    );

    g.ready(t.solo, &round("b", "main"), t.solo, Some(&via()))
        .unwrap();
    assert_eq!(
        g.verdict(second, "b", Judgment::Pass, None, Some(&via())),
        Ok(GateState::Passed)
    );
    assert_eq!(g.queued_targets().unwrap(), [(repo, "main".to_owned())]);
    assert_eq!(names(g.queue(repo, "main").unwrap()), ["x", "b"]);
}

#[test]
fn passes_in_one_second_keep_the_pass_order() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    let r = reviewer(&g, t.solo, "solo", "r");
    for branch in ["a", "b"] {
        g.ready(t.solo, &round(branch, "main"), t.solo, Some(&via()))
            .unwrap();
    }
    for branch in ["b", "a"] {
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
    let r = reviewer(&g, t.solo, "solo", "r");
    let repo = RepoId(1);
    for branch in ["a", "b"] {
        queued(&g, t.solo, t.solo, r, branch, "main");
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
    let r = reviewer(&g, t.coordinator, "코디-1", "r");
    queued(&g, t.worker, t.coordinator, r, "b", "코디 브랜치");
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
    assert_eq!(trace.role.as_deref(), Some(COORDINATOR_ROLE));
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
