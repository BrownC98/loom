use super::*;
use crate::protocol::{
    CI_ROLE, COORDINATOR_ROLE, Finding, GateState, GateVerb, Judgment, MergeEnd,
};

fn with_role(g: &Graph, parent: NodeId, name: &str, role: &str) -> NodeId {
    g.add_child(
        parent,
        NewChild {
            role: Some(role),
            ..named(name)
        },
        TreeLimits {
            max_depth: None,
            max_children: None,
        },
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
    ci: NodeId,
    coordinator: NodeId,
    worker: NodeId,
}

fn team(g: &Graph) -> Team {
    let lead = root(g, "claude");
    Team {
        lead,
        ci: with_role(g, lead, "ci", CI_ROLE),
        coordinator: with_role(g, lead, "코디-1", COORDINATOR_ROLE),
        worker: with_role(g, lead, "worker-x", "워커"),
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

#[test]
fn spawn_role_is_stored_and_picks_the_gate() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    let worker = with_role(&g, t.coordinator, "w", "워커");
    let role: Option<String> = g
        .lock()
        .query_row("SELECT role FROM nodes WHERE id = ?1", [worker.0], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(role.as_deref(), Some("워커"));
    assert_eq!(
        g.gate_for(worker).unwrap(),
        GateChoice::Coordinator {
            gate: t.coordinator,
            worktree: "/repo".into()
        }
    );
    assert_eq!(
        g.gate_for(t.coordinator).unwrap(),
        GateChoice::Ci { gate: t.ci }
    );
    let second = with_role(&g, t.lead, "ci2", CI_ROLE);
    assert!(
        matches!(g.gate_for(t.worker), Err(Refusal::ManyGates(ids)) if ids.contains(&format!("#{second}")))
    );
    g.exited(second, 0).unwrap();
    assert_eq!(g.gate_for(t.worker).unwrap(), GateChoice::Ci { gate: t.ci });
    g.done(t.ci, &report("idle"), None, &[], None, None)
        .unwrap();
    g.slept(t.ci).unwrap();
    assert_eq!(g.gate_for(t.worker).unwrap(), GateChoice::Ci { gate: t.ci });
    g.sleep_lost(t.ci).unwrap();
    assert_eq!(g.gate_for(t.worker), Err(Refusal::NoGate));
}

#[test]
fn a_round_goes_ready_block_ready_pass_merge_with_fixed_receivers() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    let reviewer = with_role(&g, t.ci, "reviewer", "검토자");
    let v = via();
    let branch = "기능";

    assert_eq!(
        g.ready(t.coordinator, &round(branch, "main"), t.ci, Some(&v)),
        Ok("ci".into())
    );
    assert_eq!(state(&g, branch), GateState::Ready);
    let to_ci = mail(&g, t.ci);
    assert_eq!(to_ci.len(), 1);
    assert!(
        to_ci[0].starts_with(
            "(from: 코디-1, kind: ready)\nbranch: 기능\nhead: c0ffee1\nworktree: /repo/wt\ntarget: main\nNext: review"
        ),
        "{}",
        to_ci[0]
    );
    assert!(matches!(
        g.ready(t.coordinator, &round(branch, "main"), t.ci, Some(&v)),
        Err(Refusal::Gate(e)) if e.state == GateState::Ready && e.allowed == [GateVerb::Verdict, GateVerb::Fail]
    ));

    assert!(matches!(
        g.verdict(t.worker, branch, Judgment::Pass, None, Some(&v)),
        Err(Refusal::NotGate { gate, .. }) if gate == "ci"
    ));
    assert_eq!(
        g.verdict(reviewer, branch, Judgment::Block, None, Some(&v)),
        Err(Refusal::NoFindings)
    );
    assert_eq!(
        g.verdict(
            reviewer,
            branch,
            Judgment::Block,
            Some("/r/findings.md"),
            Some(&v)
        ),
        Ok(GateState::Working)
    );
    let rework = mail(&g, t.coordinator);
    assert_eq!(rework.len(), 1);
    assert!(
        rework[0].starts_with(
            "(from: reviewer, kind: verdict)\nbranch: 기능\nhead: c0ffee1\njudgment: block\nfindings: /r/findings.md\n"
        ) && rework[0].contains("\nNext: fix each finding"),
        "{}",
        rework[0]
    );
    assert_eq!(
        mail(&g, t.ci).len(),
        1,
        "the gate hears its reviewer's block"
    );
    let reviewer_model: Option<String> = g
        .lock()
        .query_row("SELECT verdict ->> '$.reviewer.name' FROM gates", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(reviewer_model.as_deref(), Some("reviewer"));

    g.ready(t.coordinator, &round(branch, "main"), t.ci, Some(&v))
        .unwrap();
    mail(&g, t.ci);
    assert_eq!(
        g.verdict(t.coordinator, branch, Judgment::Pass, None, Some(&v)),
        Err(Refusal::OwnVerdict(branch.into()))
    );
    assert_eq!(
        g.verdict(t.ci, branch, Judgment::Pass, None, Some(&v)),
        Ok(GateState::Passed)
    );
    assert!(
        mail(&g, t.coordinator)[0]
            .contains("kind: verdict)\nbranch: 기능\nhead: c0ffee1\njudgment: pass")
    );
    assert_eq!(
        g.verdict(reviewer, branch, Judgment::Block, Some("/r/2.md"), Some(&v)),
        Ok(GateState::Working)
    );
    mail(&g, t.coordinator);
    mail(&g, t.ci);
    g.ready(t.coordinator, &round(branch, "main"), t.ci, Some(&v))
        .unwrap();
    g.verdict(t.ci, branch, Judgment::Pass, None, Some(&v))
        .unwrap();
    assert_eq!(
        g.verdict(reviewer, branch, Judgment::Pass, None, Some(&v)),
        Ok(GateState::Passed)
    );
    mail(&g, t.coordinator);
    mail(&g, t.ci);
    assert!(matches!(
        g.merge_start(t.coordinator, branch, Some(&v)),
        Err(Refusal::NotGate { .. })
    ));
    let task = g.merge_start(t.ci, branch, Some(&v)).unwrap();
    assert_eq!(
        (
            task.owner,
            task.target.as_str(),
            task.head.as_str(),
            task.run,
            task.from
        ),
        (t.coordinator, "main", "c0ffee1", 1, GateState::Passed)
    );
    assert!(mail(&g, t.coordinator)[0].starts_with("(from: ci, kind: merging)\nbranch: 기능"));
    let again = g.merge_start(t.ci, branch, Some(&v)).unwrap();
    assert_eq!((again.run, again.from), (2, GateState::Merging));
    mail(&g, t.coordinator);
    let ended = |sha| Ended {
        end: MergeEnd::Merged,
        merged: Some(sha),
        cause: None,
        log: Some("/l/merge.log".into()),
        lines: vec!["deploy: Confirmed".into()],
        findings: Vec::new(),
    };
    assert_eq!(
        g.merge_end(t.ci, branch, 1, ended("old0000"), Some(&v)),
        Err(Refusal::StaleRun {
            branch: branch.into(),
            run: 1
        })
    );
    assert_eq!(
        g.merge_end(t.ci, branch, 2, ended("abc1234"), Some(&v)),
        Ok(GateState::Merged)
    );
    let merged = mail(&g, t.lead);
    assert_eq!(merged.len(), 1);
    assert!(
        merged[0].starts_with(
            "(from: ci, kind: merged)\nbranch: 기능\nhead: c0ffee1\nsha: abc1234\nlog: /l/merge.log\ndeploy: Confirmed\nNext: accept"
        ),
        "{}",
        merged[0]
    );
    assert_eq!(
        mail(&g, t.coordinator).len(),
        1,
        "the owner hears the merge too"
    );
    assert_eq!(g.gate_merges(t.ci, "main").unwrap(), ["abc1234"]);
    assert_eq!(
        g.ready(t.coordinator, &round(branch, "main"), t.ci, Some(&v)),
        Ok("ci".into())
    );
    assert_eq!(state(&g, branch), GateState::Ready);
    assert_eq!(g.gate_merges(t.ci, "main").unwrap(), ["abc1234"]);
    assert!(g.gate_merges(t.ci, "코디 브랜치").unwrap().is_empty());
}

#[test]
fn a_gate_call_without_receivers_goes_to_the_caller() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    let v = via();
    g.ready(t.worker, &round("b", "main"), t.ci, Some(&v))
        .unwrap();
    g.verdict(t.ci, "b", Judgment::Pass, None, Some(&v))
        .unwrap();
    let task = g.merge_start(t.ci, "b", Some(&v)).unwrap();
    mail(&g, t.ci);
    g.remove(t.worker).unwrap();
    let ended = Ended {
        end: MergeEnd::Merged,
        merged: Some("abc1234"),
        cause: None,
        log: None,
        lines: Vec::new(),
        findings: Vec::new(),
    };
    assert_eq!(
        g.merge_end(t.ci, "b", task.run, ended, Some(&v)),
        Ok(GateState::Merged)
    );
    let kept = mail(&g, t.ci);
    assert_eq!(kept.len(), 1);
    assert!(
        kept[0].starts_with(
            "(from: ci, kind: merged)\nbranch: b\nhead: c0ffee1\nsha: abc1234\nNo other node"
        ),
        "{}",
        kept[0]
    );
    let recent = g.recent_events().unwrap();
    assert!(recent.iter().any(|r| matches!(
        &r.entry,
        Entry::Message { gate: Some(trace), .. } if trace.to == Some(GateState::Merged)
    )));
}

#[test]
fn fail_and_no_round_and_unknown_callers_are_refused_or_routed_to_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    let v = via();
    assert_eq!(
        g.verdict(t.ci, "없음", Judgment::Pass, None, Some(&v)),
        Err(Refusal::NoRound("없음".into()))
    );
    assert_eq!(
        g.fail(t.ci, "없음", "x", Some(&v)),
        Err(Refusal::NoRound("없음".into()))
    );
    g.ready(t.worker, &round("b", "main"), t.ci, Some(&v))
        .unwrap();
    mail(&g, t.ci);
    assert!(matches!(
        g.fail(t.worker, "b", "x", Some(&v)),
        Err(Refusal::NotGate { .. })
    ));
    assert_eq!(
        g.fail(t.ci, "b", "deploy is down", Some(&v)),
        Ok(GateState::Failed)
    );
    let to_root = mail(&g, t.lead);
    assert!(
        to_root[0].starts_with(
            "(from: ci, kind: failed)\nbranch: b\nhead: c0ffee1\ncause: deploy is down\nNext: decide"
        ),
        "{}",
        to_root[0]
    );
    assert_eq!(
        mail(&g, t.worker).len(),
        1,
        "the owner hears the failure too"
    );
    assert_eq!(
        g.ready(t.worker, &round("b", "main"), t.ci, Some(&v)),
        Ok("ci".into())
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
    g.ready(t.coordinator, &round("b", "main"), t.ci, Some(&v))
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
    g.note(t.worker, "ci", Kind::Queue, "hi", Some(&send_via))
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
        .note(t.worker, "ci", Kind::Queue, "READY old", None)
        .unwrap();
    g.lock()
        .execute(
            "UPDATE events SET at = at - 10 WHERE message = ?1",
            [before.id.0],
        )
        .unwrap();
    g.ready(t.worker, &round("b", "main"), t.ci, Some(&v))
        .unwrap();
    g.lock()
        .execute("UPDATE gates SET opened = opened - 5", [])
        .unwrap();
    let raw = g
        .note(t.worker, "ci", Kind::Queue, "READY b /x", None)
        .unwrap();
    g.note(stranger, "ci", Kind::Queue, "READY other", None)
        .unwrap();
    g.note(t.lead, "ci", Kind::Queue, "MERGED other", None)
        .unwrap();
    g.note(t.worker, "ci", Kind::Queue, "Ready for review", None)
        .unwrap();
    g.human_typed(t.worker, "READY now").unwrap();
    let since = g.merge_task(t.worker, "b").unwrap().since;
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
fn a_stopped_merge_stays_merging_and_only_the_gate_hears_it() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    let v = via();
    g.ready(t.worker, &round("b", "main"), t.ci, Some(&v))
        .unwrap();
    g.verdict(t.ci, "b", Judgment::Pass, None, Some(&v))
        .unwrap();
    let task = g.merge_start(t.ci, "b", Some(&v)).unwrap();
    mail(&g, t.ci);
    mail(&g, t.worker);
    let ended = Ended {
        end: MergeEnd::Stopped,
        merged: None,
        cause: Some("push was rejected".into()),
        log: None,
        lines: Vec::new(),
        findings: Vec::new(),
    };
    assert_eq!(
        g.merge_end(t.ci, "b", task.run, ended, Some(&v)),
        Ok(GateState::Merging)
    );
    let kept = mail(&g, t.ci);
    assert_eq!(kept.len(), 1);
    assert!(
        kept[0].starts_with(
            "(from: ci, kind: merging)\nbranch: b\nhead: c0ffee1\ncause: push was rejected\nNo other node"
        ),
        "{}",
        kept[0]
    );
    assert!(mail(&g, t.worker).is_empty());
    assert!(mail(&g, t.lead).is_empty());
    assert_eq!(
        g.merge_start(t.ci, "b", Some(&v)).unwrap().run,
        task.run + 1
    );
}

#[test]
fn a_branch_task_comes_from_the_owner_of_its_gate_round() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    assert_eq!(g.branch_task(t.ci, "new-name").unwrap(), None);
    g.ready(t.worker, &round("new-name", "main"), t.ci, Some(&via()))
        .unwrap();
    assert_eq!(
        g.branch_task(t.ci, "new-name").unwrap(),
        Some(("/repo".to_owned(), "worker-x".to_owned()))
    );
    assert_eq!(g.branch_task(t.ci, "old-name").unwrap(), None);
    g.remove(t.worker).unwrap();
    assert_eq!(g.branch_task(t.ci, "new-name").unwrap(), None);
}

#[test]
fn an_ungated_ready_reaches_the_gate_without_a_gate_row() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    let v = via();
    assert!(!g.ready_sent(t.worker, "b", "c0ffee1").unwrap());
    assert_eq!(
        g.announce_ready(t.worker, &round("b", "main"), t.ci, Some(&v)),
        Ok("ci".into())
    );
    let to_ci = mail(&g, t.ci);
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
