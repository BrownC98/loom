//! 병합 관문 상태(`gate.rs`): 관문 고르기, 전이와 받는 노드, 흔적, audit. 전이 표 자체는 `protocol` 시험이 본다.

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

/// 총괄 아래 CI 담당자와 코디, 코디 아래 워커.
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
    taken.into_iter().map(|d| d.body).collect()
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
    // 코디 밑 워커의 관문은 코디, 코디의 관문은 CI 담당자다.
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
    // 끝난 CI 담당자는 고르지 않는다. 산 것이 둘이면 거절한다.
    let second = with_role(&g, t.lead, "ci2", CI_ROLE);
    assert!(
        matches!(g.gate_for(t.worker), Err(Refusal::ManyGates(ids)) if ids.contains(&format!("#{second}")))
    );
    g.exited(second, 0).unwrap();
    assert_eq!(g.gate_for(t.worker).unwrap(), GateChoice::Ci { gate: t.ci });
    g.exited(t.ci, 0).unwrap();
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
        to_ci[0].starts_with("gate 기능: ready for review\nowner: 코디-1"),
        "{}",
        to_ci[0]
    );
    assert!(to_ci[0].contains("head: c0ffee1") && to_ci[0].contains("target: main"));
    // 같은 차례에 두 번 ready하지 않는다.
    assert!(matches!(
        g.ready(t.coordinator, &round(branch, "main"), t.ci, Some(&v)),
        Err(Refusal::Gate(e)) if e.state == GateState::Ready && e.allowed == [GateVerb::Verdict, GateVerb::Fail]
    ));

    // 관문도 검토자도 아닌 노드는 판정하지 못한다. block은 결함 파일이 있어야 한다.
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
            "gate 기능: verdict block (rework)\nfindings: /r/findings.md\nNext: fix each finding"
        ),
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
    // 담당은 자기 브랜치를 판정하지 못한다(검토 P2-1).
    assert_eq!(
        g.verdict(t.coordinator, branch, Judgment::Pass, None, Some(&v)),
        Err(Refusal::OwnVerdict(branch.into()))
    );
    assert_eq!(
        g.verdict(t.ci, branch, Judgment::Pass, None, Some(&v)),
        Ok(GateState::Passed)
    );
    assert!(mail(&g, t.coordinator)[0].starts_with("gate 기능: verdict pass"));
    // 병합 시작 전에는 둘째 검토자가 pass를 block으로 바꾼다(검토 P2-7).
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
    assert!(mail(&g, t.coordinator)[0].starts_with("gate 기능: merge started"));
    // 앞 실행이 보고 없이 끝났으면 같은 관문이 새 번호로 다시 시작한다. 앞 번호의 보고는 받지 않는다(검토 P2-4).
    let again = g.merge_start(t.ci, branch, Some(&v)).unwrap();
    assert_eq!((again.run, again.from), (2, GateState::Merging));
    mail(&g, t.coordinator);
    let ended = |sha| Ended {
        end: MergeEnd::Merged,
        merged: Some(sha),
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
            "gate 기능: merged\nmerged commit: abc1234\ndeploy: Confirmed\nNext: accept"
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
    // 병합한 브랜치 이름을 다시 쓰면 새 차례다. 앞 병합 커밋은 관문 밖 커밋 검사의 기준으로 남는다(검토 P1-3).
    assert_eq!(
        g.ready(t.coordinator, &round(branch, "main"), t.ci, Some(&v)),
        Ok("ci".into())
    );
    assert_eq!(state(&g, branch), GateState::Ready);
    assert_eq!(g.gate_merges(t.ci, "main").unwrap(), ["abc1234"]);
    assert!(g.gate_merges(t.ci, "코디 브랜치").unwrap().is_empty());
}

/// 담당을 지워 받을 노드가 없어도 성공한 관문 호출은 흔적을 남긴다: 부른 노드가 받는다(검토 C6).
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
        kept[0].starts_with("gate b: merged\nmerged commit: abc1234\nNo other node"),
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
        to_root[0].starts_with("gate b: failed\ncause: deploy is down\nNext: decide"),
        "{}",
        to_root[0]
    );
    assert_eq!(
        mail(&g, t.worker).len(),
        1,
        "the owner hears the failure too"
    );
    // 원인을 고친 뒤 담당이 새 차례를 연다.
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
    // 손으로 보낸 메시지도 입구를 남긴다.
    let send_via = Via {
        tool: "loom send".into(),
        version: None,
    };
    g.send(t.worker, "ci", Kind::Queue, "hi", Some(&send_via))
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
    // 앞 차례의 글은 이번 차례의 audit에 들지 않는다(검토 P1-1).
    let before = g
        .send(t.worker, "ci", Kind::Queue, "READY old", None)
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
        .send(t.worker, "ci", Kind::Queue, "READY b /x", None)
        .unwrap();
    // 담당의 자손이 아닌 노드(다른 뿌리, 담당의 형제·부모)는 이 브랜치와 관계없다(검토 P2-5).
    g.send(stranger, "ci", Kind::Queue, "READY other", None)
        .unwrap();
    g.send(t.lead, "ci", Kind::Queue, "MERGED other", None)
        .unwrap();
    g.send(t.worker, "ci", Kind::Queue, "Ready for review", None)
        .unwrap();
    let since = g.merge_task(t.worker, "b").unwrap().since;
    // 관문 동사가 보낸 메시지(`gate ...`)와 나무 밖 노드의 글은 찾지 않는다.
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
    // 시작점이 남은 가장 오래된 이벤트보다 앞이면 구간이 비었다.
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

/// 다시 부르면 될 일로 멈춘 병합은 `Merging`에 남고, 관문 자신이 기록을 받는다. 담당은 반려를 받지 않는다(재작업 3 결정 3).
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
        lines: vec!["cause: push was rejected".into()],
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
            "gate b: merge stopped (still merging)\ncause: push was rejected\nNo other node"
        ),
        "{}",
        kept[0]
    );
    assert!(mail(&g, t.worker).is_empty());
    assert!(mail(&g, t.lead).is_empty());
    // 관문은 다시 병합한다.
    assert_eq!(
        g.merge_start(t.ci, "b", Some(&v)).unwrap().run,
        task.run + 1
    );
}

/// `loom verdict`·`merge`의 기본 경로는 관문 줄 담당의 작업 폴더다(데몬 `agent_task`). 브랜치 이름을 바꾸고 `loom ready`해도 처음 작업이다.
#[test]
fn a_renamed_branch_round_finds_the_owner_task() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let t = team(&g);
    // 담당의 작업 폴더는 띄울 때 정했다(`named`가 노드 이름으로 둔다). 브랜치를 `new-name`으로 바꾼 뒤 그 이름으로 보낸다.
    g.ready(t.worker, &round("new-name", "main"), t.ci, Some(&via()))
        .unwrap();
    let owner = g.merge_task(t.ci, "new-name").unwrap().owner;
    assert_eq!(owner, t.worker);
    assert_eq!(g.task_of(owner).unwrap().as_deref(), Some("worker-x"));
    assert!(matches!(
        g.merge_task(t.ci, "old-name"),
        Err(Refusal::NoRound(_))
    ));
}
