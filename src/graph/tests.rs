use std::path::{Path, PathBuf};

use super::*;
use crate::db;
use crate::profile::ServerLink;
use crate::worktree::RepoId;

/// `home()`의 저장소 줄이 있는 그래프. 노드는 목록에 있는 저장소에만 든다.
fn open(path: &Path) -> Graph {
    let db = db::open(path).unwrap();
    db.lock()
        .execute(
            "INSERT OR IGNORE INTO repos (id, path) VALUES (1, '/repo')",
            [],
        )
        .unwrap();
    Graph::open(db).unwrap()
}

/// 테스트 노드가 도는 자리.
fn home() -> Place {
    Place {
        repo: RepoId(1),
        worktree: "/repo".into(),
    }
}

/// 노드마다 다른 토큰.
fn launched() -> Launched {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    Launched {
        token: format!(
            "t{}",
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ),
        server: None,
        account: None,
    }
}

fn started(_: NodeId) -> Result<(), String> {
    Ok(())
}

/// 에이전트 id가 `agent`인 루트. 이름은 `<agent>-1`이다.
fn root(g: &Graph, agent: &str) -> NodeId {
    g.add_root(&home(), Some(AgentId(agent.into())), &launched(), started)
        .unwrap()
}

/// 지시문 없이 부모 워크트리에 드는 셸 자식.
fn named(name: &str) -> NewChild<'_> {
    NewChild {
        name,
        worktree: None,
        agent: None,
        brief: "",
    }
}

fn child(g: &Graph, parent: NodeId, name: &str) -> NodeId {
    g.add_child(
        parent,
        named(name),
        TreeLimits::default(),
        &launched(),
        started,
    )
    .unwrap()
}

/// 메시지 하나를 꺼내고 받음을 확인한다. 보낸 노드, 방식, 본문을 돌려준다.
fn take(g: &Graph, node: NodeId) -> Option<(String, Kind, String)> {
    let d = g.take(node, Kind::ALL, 0).unwrap().pop()?;
    g.ack(node, &[d.id]).unwrap();
    Some((d.from, d.kind, d.body))
}

fn delivery(from: &str, body: &str) -> Option<(String, Kind, String)> {
    Some((from.into(), Kind::Queue, body.into()))
}

/// 확인 없이 둔 메시지를 다시 꺼낼 수 있게 꺼낸 시각을 `REDELIVER_AFTER`만큼 앞으로 옮긴다.
fn expire(g: &Graph) {
    g.lock()
        .execute(
            "UPDATE messages SET sent_at = sent_at - ?1",
            [REDELIVER_AFTER.as_secs() as i64],
        )
        .unwrap();
}

fn bodies<'a>(taken: impl IntoIterator<Item = &'a Delivery>) -> Vec<&'a str> {
    taken.into_iter().map(|d| d.body.as_str()).collect()
}

fn held(g: &Graph, id: NodeId) -> u32 {
    g.lock()
        .query_row(
            "SELECT count(*) FROM messages WHERE recipient IS NULL AND sender = ?1",
            [id.0],
            |row| row.get(0),
        )
        .unwrap()
}

fn view(g: &Graph, id: NodeId) -> NodeView {
    g.subscribe()
        .unwrap()
        .0
        .into_iter()
        .find(|v| v.id == id)
        .unwrap()
}

#[test]
fn model_change_reaches_the_view_once() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let node = root(&g, "codex");
    let (_, mut events) = g.subscribe().unwrap();
    assert_eq!((view(&g, node).model, view(&g, node).effort), (None, None));
    let model = Model {
        model: Some("gpt-6.1-sol".into()),
        effort: Some("medium".into()),
    };
    g.set_model(node, &model).unwrap();
    g.set_model(node, &model).unwrap();
    let Ok(GraphEvent::Node(changed)) = events.try_recv() else {
        panic!("Node 이벤트가 와야 한다")
    };
    assert_eq!(
        (changed.model.as_deref(), changed.effort.as_deref()),
        (Some("gpt-6.1-sol"), Some("medium"))
    );
    // 같은 값은 다시 알리지 않는다.
    assert!(events.try_recv().is_err());
    g.set_model(node, &Model::default()).unwrap();
    assert!(matches!(events.try_recv(), Ok(GraphEvent::Node(v)) if v.effort.is_none()));
}

#[test]
fn signal_wait_has_one_entry_and_clears_by_source_key_and_exit() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "claude");
    let worker = child(&g, lead, "w");
    let waits = |id| {
        view(&g, id)
            .waits
            .into_iter()
            .map(|w| (w.kind, w.text))
            .collect::<Vec<_>>()
    };

    // 턴 끝 검사는 대기 없고 `loom done`을 부르지 않은 루트만 본다.
    assert_eq!(g.unreported_root(lead), Ok(true));
    assert_eq!(g.unreported_root(worker), Ok(false));
    // 다른 신호는 자기가 건 대기만 거둔다.
    let trust = (WaitKind::Trust, "Trust?");
    g.set_wait(lead, WaitSource::Screen, Some(trust)).unwrap();
    assert_eq!(g.unreported_root(lead), Ok(false));
    g.set_wait(lead, WaitSource::Hook, None).unwrap();
    assert_eq!(waits(lead), [(WaitKind::Trust, "Trust?".into())]);
    // 사람의 키 입력이 신호의 대기를 지운다.
    g.typed(lead).unwrap();
    assert_eq!(waits(lead), []);

    let mut events = g.subscribe().unwrap().1;
    g.set_wait(worker, WaitSource::Codex, Some((WaitKind::Approval, "")))
        .unwrap();
    let since = view(&g, worker).waits[0].since;
    // 같은 대기는 다시 알리지 않고 시작 시각을 지킨다.
    g.set_wait(worker, WaitSource::Codex, Some((WaitKind::Approval, "")))
        .unwrap();
    assert!(matches!(events.try_recv(), Ok(GraphEvent::Node(v)) if v.waits.len() == 1));
    assert!(events.try_recv().is_err());
    assert_eq!(view(&g, worker).waits[0].since, since);
    g.set_wait(worker, WaitSource::Codex, None).unwrap();
    assert_eq!(waits(worker), []);

    // 프로세스가 끝나면 지운다.
    g.set_wait(worker, WaitSource::Screen, Some((WaitKind::Trust, "t")))
        .unwrap();
    g.exited(worker, 0).unwrap();
    assert_eq!(waits(worker), []);

    // 훅 대기: 다음 훅이 거두고(`loom hook`), 사람이 받은편지함 [×]로도 지운다(`DELETE /api/waits/{wait}`).
    let permission = Some((WaitKind::Approval, "Claude needs your permission"));
    g.set_wait(lead, WaitSource::Hook, permission).unwrap();
    g.set_wait(lead, WaitSource::Hook, None).unwrap();
    assert_eq!(waits(lead), []);
    g.set_wait(lead, WaitSource::Hook, permission).unwrap();
    g.dismiss(view(&g, lead).waits[0].id).unwrap();
    assert_eq!(waits(lead), []);
}

/// 노드 하나가 질문 여럿을 연다(목업 M28). 새 질문은 앞 질문을 지우지 않고, 신호의 대기와 따로 산다.
/// `loom answered`는 번호로 닫는다. 번호 없이는 열린 질문이 하나일 때만 닫는다.
#[test]
fn root_dones_stay_in_the_inbox_until_the_human_confirms_and_never_hide_a_wait() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "claude");
    let worker = child(&g, lead, "w");
    let done = |id| {
        view(&g, id)
            .waits
            .into_iter()
            .map(|w| (w.kind, w.instruction, w.text, w.images))
            .collect::<Vec<_>>()
    };
    let kinds = |id| done(id).into_iter().map(|w| w.0).collect::<Vec<_>>();

    // 사람이 띄운 노드는 사람의 지시 없이 끝내지 못한다. 거절하면 상태도 그대로다.
    assert_eq!(g.done(lead, "끝", None, &[], None), Err(Refusal::NoAsked));
    assert_eq!(
        g.done(lead, "끝", Some("  "), &[], None),
        Err(Refusal::NoAsked)
    );
    assert_eq!(view(&g, lead).state, NodeState::Running);
    // 지시·결과는 전문을 남긴다. 그림은 경로를 남긴다.
    let shot = PathBuf::from("/tmp/loom-paste/1/image.png");
    g.done(
        lead,
        "버튼을 고쳤다.\n시험 통과",
        Some(" 버튼 고쳐\n자세히는… "),
        std::slice::from_ref(&shot),
        None,
    )
    .unwrap();
    assert_eq!(
        done(lead),
        [(
            WaitKind::Done,
            Some("버튼 고쳐\n자세히는…".into()),
            "버튼을 고쳤다.\n시험 통과".into(),
            vec!["/tmp/loom-paste/1/image.png".to_owned()]
        )]
    );
    let first = view(&g, lead).waits[0].id;
    assert_eq!(g.done_image(first, 0), Ok(shot.clone()));
    assert_eq!(g.done_image(first, 1), Err(Refusal::NoWait(first)));
    // 완료는 [확인](dismiss)만 지운다. 키 입력, 다른 신호, 다시 띄우기로는 남는다.
    g.typed(lead).unwrap();
    g.clear_wait(lead).unwrap();
    assert_eq!(done(lead).len(), 1);
    // 승인 창이 와도 완료를 덮지 않는다. 함께 보이고 승인 창이 먼저다.
    let permission = (WaitKind::Approval, "Allow Bash?");
    g.set_wait(lead, WaitSource::Hook, Some(permission))
        .unwrap();
    assert_eq!(kinds(lead), [WaitKind::Approval, WaitKind::Done]);
    g.set_wait(lead, WaitSource::Hook, None).unwrap();
    // 새 일로 다시 일하다 끝내도 확인하지 않은 앞 완료는 남는다. 완료 차례대로 보인다(2026-10-22 사람 보고 "완료 알림이 확인 안눌러도 없어지는 버그가 있음").
    g.human_input(lead).unwrap();
    g.done(lead, "또 고쳤다.", Some("또 고쳐"), &[], None)
        .unwrap();
    assert_eq!(
        done(lead),
        [
            (
                WaitKind::Done,
                Some("버튼 고쳐\n자세히는…".into()),
                "버튼을 고쳤다.\n시험 통과".into(),
                vec!["/tmp/loom-paste/1/image.png".to_owned()]
            ),
            (
                WaitKind::Done,
                Some("또 고쳐".into()),
                "또 고쳤다.".into(),
                vec![]
            )
        ]
    );
    assert_eq!(g.done_image(first, 0), Ok(shot));
    // 완료 줄의 입력창: 이어서 맡기는 일은 사람이 보낸 메시지로 가고 그 완료만 사라진다.
    let again = view(&g, lead).waits[1].id;
    assert!(view(&g, lead).waits[1].answerable);
    g.answer(again, "하나 더", &[]).unwrap();
    assert_eq!(
        view(&g, lead)
            .waits
            .iter()
            .map(|w| w.id)
            .collect::<Vec<_>>(),
        [first]
    );
    let (from, _, body) = take(&g, lead).unwrap();
    assert_eq!(from, HUMAN);
    assert_eq!(
        body,
        "New instruction from the human in the loom inbox, after your done report: 하나 더"
    );
    assert_eq!(g.answer(again, "또", &[]), Err(Refusal::NoWait(again)));
    // [확인]은 그 완료만 지운다.
    g.done(lead, "또 고쳤다.", Some("하나 더"), &[], None)
        .unwrap();
    let third = view(&g, lead).waits[1].id;
    g.dismiss(first).unwrap();
    assert_eq!(
        view(&g, lead)
            .waits
            .iter()
            .map(|w| w.id)
            .collect::<Vec<_>>(),
        [third]
    );
    assert_eq!(g.done_image(first, 0), Err(Refusal::NoWait(first)));
    // 질문과 완료가 섞이면 둘 다 보이고 질문이 먼저다. 질문을 [×]로 닫아도 완료는 남는다.
    g.ask(lead, "병합할까요?", None).unwrap();
    assert_eq!(kinds(lead), [WaitKind::Question, WaitKind::Done]);
    g.dismiss(view(&g, lead).waits[0].id).unwrap();
    assert_eq!(kinds(lead), [WaitKind::Done]);
    g.dismiss(view(&g, lead).waits[0].id).unwrap();
    assert_eq!(done(lead), []);

    // 문서가 붙은 완료: 화면은 ID와 제목만 받고, 데몬은 [확인] 전까지 파일을 준다.
    let doc = WaitDoc {
        id: "R4".into(),
        title: "감사".into(),
    };
    let report = PathBuf::from("/tmp/audit.md");
    let attached = Attached {
        doc: doc.clone(),
        path: report.clone(),
    };
    g.done(lead, "감사했다.", Some("감사해"), &[], Some(attached))
        .unwrap();
    let with_doc = view(&g, lead).waits[0].clone();
    assert_eq!(with_doc.doc, Some(doc.clone()));
    assert_eq!(g.wait_doc(with_doc.id), Ok(report.clone()));
    g.dismiss(with_doc.id).unwrap();
    assert_eq!(g.wait_doc(with_doc.id), Err(Refusal::NoWait(with_doc.id)));

    // 자식의 완료는 부모에게만 간다. 받은편지함에 걸지 않는다. 문서 링크는 받은편지함에만 있어 거절한다.
    g.done(worker, "고쳤다.", Some("버튼 고쳐"), &[], None)
        .unwrap();
    assert_eq!(done(worker), []);
    let attached = Attached { doc, path: report };
    assert_eq!(
        g.done(worker, "고쳤다.", None, &[], Some(attached)),
        Err(Refusal::DocHasParent)
    );
}

#[test]
fn a_node_keeps_several_questions_and_closes_them_by_id() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "claude");
    let worker = child(&g, lead, "w");
    let ids = |id| {
        view(&g, id)
            .waits
            .into_iter()
            .map(|w| w.id)
            .collect::<Vec<_>>()
    };

    // 사람에게는 사람이 띄운 노드만 묻는다.
    assert_eq!(g.ask(worker, "A?", None), Err(Refusal::HasParent));
    assert_eq!(g.ask(lead, "  ", None), Err(Refusal::EmptyQuestion));
    // `loom ask` 질문은 줄바꿈까지 전문을 남긴다.
    let q1 = g.ask(lead, "A or B?\nmore", None).unwrap();
    let q2 = g.ask(lead, "C?", None).unwrap();
    let q3 = g.ask(lead, "D?", None).unwrap();
    assert_eq!(g.unreported_root(lead), Ok(false));
    assert_eq!(
        g.open_questions(lead),
        Ok(vec![
            OpenQuestion {
                id: q1,
                text: "A or B?\nmore".into()
            },
            OpenQuestion {
                id: q2,
                text: "C?".into()
            },
            OpenQuestion {
                id: q3,
                text: "D?".into()
            },
        ])
    );
    // 신호의 대기는 질문 뒤에 온다. 키 입력과 신호는 질문을 지우지 않는다.
    g.set_wait(lead, WaitSource::Hook, Some((WaitKind::Approval, "Bash")))
        .unwrap();
    let hook = view(&g, lead).waits[3].id;
    assert_eq!(ids(lead), [q1, q2, q3, hook]);
    g.typed(lead).unwrap();
    assert_eq!(ids(lead), [q1, q2, q3]);

    // 번호 없는 `loom answered`는 여럿이 열려 있으면 거절한다.
    assert_eq!(
        g.answered(lead, &[]),
        Err(Refusal::SeveralOpen(vec![q1, q2, q3]))
    );
    assert_eq!(g.answered(lead, &[q2, q3]), Ok(vec![q2, q3]));
    // 다른 노드의 질문이나 닫힌 질문은 닫지 못한다.
    assert_eq!(g.answered(lead, &[q2]), Err(Refusal::NoWait(q2)));
    assert_eq!(g.answered(lead, &[]), Ok(vec![q1]));
    assert_eq!(g.answered(lead, &[]), Ok(vec![]));
    assert_eq!(g.unreported_root(lead), Ok(true));

    // [×]는 그 질문 하나만, 재시작은 모두 지운다.
    let q4 = g.ask(lead, "E?", None).unwrap();
    let q5 = g.ask(lead, "F?", None).unwrap();
    g.dismiss(q4).unwrap();
    assert_eq!(ids(lead), [q5]);
    // 닫은 질문은 사람이 보낸 짧은 큐 메시지로 그 노드에 알린다.
    assert_eq!(
        take(&g, lead),
        delivery("human", &format!("{q4} was closed without an answer."))
    );
    assert_eq!(g.dismiss(q4), Err(Refusal::NoWait(q4)));
    g.ask(lead, "G?", None).unwrap();
    g.clear_wait(lead).unwrap();
    assert_eq!(ids(lead), []);
}

/// 대기 번호는 데몬 전체에서 늘기만 한다. 지운 번호도, 같은 질문을 다시 물어도 새 번호다. 다시 열어도 이어진다.
/// 사람이 화면에서 답하면 사람이 보낸 큐 메시지가 그 노드에 가고 질문은 바로 사라진다. 문서는 열린 질문의 것만 준다.
#[test]
fn waits_get_new_ids_and_the_human_answers_from_the_inbox() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("loom.db");
    let g = open(&path);
    let lead = root(&g, "총괄");
    let other = root(&g, "다른");
    let id = |node| view(&g, node).waits.first().map(|w| w.id);

    let first = g.ask(lead, "A or B?", None).unwrap();
    let screen = g
        .set_wait(other, WaitSource::Screen, Some((WaitKind::Trust, "Trust?")))
        .map(|()| id(other).unwrap())
        .unwrap();
    assert!(screen.0 > first.0);
    assert!(!view(&g, other).waits[0].answerable);
    g.clear_wait(lead).unwrap();
    let again = g.ask(lead, "A or B?", None).unwrap();
    assert!(again.0 > screen.0);
    assert!(view(&g, lead).waits[0].answerable);

    // 다른 신호의 대기와 지운 질문에는 답하지 못한다.
    assert_eq!(g.answer(screen, "y", &[]), Err(Refusal::NoWait(screen)));
    assert_eq!(g.answer(first, "A", &[]), Err(Refusal::NoWait(first)));
    assert_eq!(g.answer(again, "  ", &[]), Err(Refusal::EmptyAnswer));
    assert_eq!(g.wait_doc(again), Err(Refusal::NoWait(again)));

    g.answer(again, "A", &[]).unwrap();
    assert_eq!(id(lead), None);
    let (from, kind, body) = take(&g, lead).unwrap();
    assert_eq!((from.as_str(), kind), (HUMAN, Kind::Queue));
    assert!(
        body.starts_with(&format!(
            "Answer to loom ask question {again} (A or B?): A\n"
        )),
        "{body}"
    );
    assert_eq!(g.answer(again, "A", &[]), Err(Refusal::NoWait(again)));

    // 그림만 보내도 답이다. 경로는 작은따옴표로 묶어 한 줄에 붙는다.
    let shot = g.ask(lead, "화면을 붙여 주세요", None).unwrap();
    let images = [
        PathBuf::from("/tmp/loom-paste/1/image.png"),
        PathBuf::from("/tmp/loom-paste/2/it's 1.png"),
    ];
    g.answer(shot, " ", &images).unwrap();
    let (_, _, body) = take(&g, lead).unwrap();
    assert!(
        body.starts_with(&format!(
            "Answer to loom ask question {shot} (화면을 붙여 주세요): \nImages: '/tmp/loom-paste/1/image.png' '/tmp/loom-paste/2/it'\\''s 1.png'\n"
        )),
        "{body}"
    );

    // 문서가 붙은 질문: 화면은 ID와 제목만 받고, 데몬은 열린 동안만 파일을 준다.
    let doc = WaitDoc {
        id: "M3".into(),
        title: "목업".into(),
    };
    let attached = Attached {
        doc: doc.clone(),
        path: dir.path().join("m.html"),
    };
    let with_doc = g.ask(lead, "승인할까요?", Some(attached)).unwrap();
    assert_eq!(view(&g, lead).waits[0].doc, Some(doc));
    assert_eq!(g.wait_doc(with_doc), Ok(dir.path().join("m.html")));
    g.answered(lead, &[]).unwrap();
    assert_eq!(g.wait_doc(with_doc), Err(Refusal::NoWait(with_doc)));

    drop(g);
    let g = open(&path);
    assert!(g.ask(lead, "다시?", None).unwrap().0 > with_doc.0);
}

#[test]
fn parent_and_child_exchange_messages() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let worker = g
        .add_child(
            lead,
            NewChild {
                brief: "node_host를 고쳐라",
                ..named("구현")
            },
            TreeLimits::default(),
            &launched(),
            started,
        )
        .unwrap();

    assert_eq!(take(&g, worker), delivery("총괄-1", "node_host를 고쳐라"));
    g.send(worker, "총괄-1", Kind::Queue, "질문 하나").unwrap();
    assert_eq!(view(&g, lead).unread, 1);
    assert_eq!(take(&g, lead), delivery("구현", "질문 하나"));
    assert_eq!(view(&g, lead).unread, 0);
    g.send(lead, "구현", Kind::Queue, "답").unwrap();
    assert_eq!(take(&g, worker), delivery("총괄-1", "답"));
    assert_eq!(take(&g, worker), None);
}

#[test]
fn messages_go_to_any_node_but_steer_only_between_parent_and_child() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let a = child(&g, lead, "a");
    let b = child(&g, lead, "b");
    let a1 = child(&g, a, "a1");

    assert_eq!(g.send(a, "b", Kind::Queue, "x").unwrap().recipient, Some(b));
    assert_eq!(take(&g, b), delivery("a", "x"));
    g.send(lead, "a1", Kind::Queue, "y").unwrap();
    assert_eq!(take(&g, a1), delivery("총괄-1", "y"));
    g.send(a1, "총괄-1", Kind::Queue, "z").unwrap();
    assert_eq!(take(&g, lead), delivery("a1", "z"));

    for kind in [Kind::Steer, Kind::Interrupt] {
        assert!(matches!(g.send(a, "b", kind, "x"), Err(Refusal::NotRelated(n)) if n == "b"));
        assert!(matches!(
            g.send(lead, &format!("#{}", a1.0), kind, "x"),
            Err(Refusal::NotRelated(_))
        ));
        // 자식도 부모를 조정·중단한다.
        assert_eq!(g.send(a1, "a", kind, "x").unwrap().recipient, Some(a));
    }
    g.send(lead, "a", Kind::Steer, "x").unwrap();
    assert!(matches!(
        g.send(a, "a", Kind::Queue, "x"),
        Err(Refusal::NotFound(_))
    ));
}

/// 다른 저장소의 두 뿌리 노드. `repo 2`를 목록에 더한다.
fn two_repos(g: &Graph) -> (NodeId, NodeId) {
    g.lock()
        .execute("INSERT INTO repos (id, path) VALUES (2, '/src/other')", [])
        .unwrap();
    let left = root(g, "claude");
    let other = Place {
        repo: RepoId(2),
        worktree: "/ws/other/화면".into(),
    };
    let right = g
        .add_root(&other, Some(AgentId("codex".into())), &launched(), started)
        .unwrap();
    (left, right)
}

#[tokio::test]
async fn roots_in_different_repos_send_and_reply() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let (left, right) = two_repos(&g);

    let question = g.send(left, "codex-1", Kind::Queue, "QUESTION").unwrap();
    assert_eq!(question.recipient, Some(right));
    let got = g.take(right, Kind::ALL, 0).unwrap().pop().unwrap();
    assert_eq!(
        (got.from.as_str(), got.body.as_str()),
        ("claude-1", "QUESTION")
    );
    g.ack(right, &[got.id]).unwrap();
    g.reply(right, question.id, "ANSWER").unwrap();
    let answer = g.wait(left, Some(question.id)).await.unwrap();
    assert_eq!(
        (answer.body.as_str(), answer.reply_to),
        ("ANSWER", Some(question.id))
    );
    // 번호로도 보낸다.
    g.send(right, &format!("#{}", left.0), Kind::Queue, "by id")
        .unwrap();
}

#[test]
fn messages_between_unrelated_nodes_pulse_as_hops() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let (left, right) = two_repos(&g);
    let w = child(&g, right, "워커");
    let (_, mut events) = g.subscribe().unwrap();
    let pulses = |events: &mut broadcast::Receiver<GraphEvent>| {
        std::iter::from_fn(|| events.try_recv().ok())
            .filter_map(|e| match e {
                GraphEvent::Hop { from, to } => Some(format!("hop {from}->{to}")),
                GraphEvent::Message { child, toward } => Some(format!("{child} {toward:?}")),
                _ => None,
            })
            .collect::<Vec<_>>()
    };

    g.send(left, "워커", Kind::Queue, "x").unwrap();
    g.send(w, "codex-1", Kind::Queue, "y").unwrap();
    assert_eq!(
        pulses(&mut events),
        [format!("hop {left}->{w}"), format!("{w} Parent")]
    );
}

#[test]
fn messages_between_unrelated_nodes_are_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let (left, right) = two_repos(&g);
    g.send(left, "codex-1", Kind::Queue, "hi").unwrap();
    take(&g, right);

    let message = |step| {
        (
            Entry::Message {
                how: Kind::Queue,
                from: who(left, "claude-1"),
                to: Some(who(right, "codex-1")),
                step,
            },
            Some("hi".to_owned()),
        )
    };
    let records = recorded(&g);
    assert_eq!(
        records[records.len() - 2..],
        [
            message(events::Step::Sent),
            message(events::Step::Delivered)
        ]
    );
}

#[test]
fn a_name_shared_by_live_nodes_is_refused_with_candidates() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let (left, right) = two_repos(&g);
    let sender = root(&g, "shell");
    let w1 = child(&g, left, "워커");
    let w2 = child(&g, right, "워커");

    let refused = g.send(sender, "워커", Kind::Queue, "x").unwrap_err();
    assert_eq!(
        refused,
        Refusal::Ambiguous {
            name: "워커".into(),
            candidates: format!("#{w1} 워커 (repo/repo), #{w2} 워커 (other/화면)"),
        }
    );
    // 프로세스가 끝난 노드는 후보가 아니다. 모두 끝났으면 끝났다고 알린다.
    g.exited(w2, 0).unwrap();
    assert_eq!(
        g.send(sender, "워커", Kind::Queue, "x").unwrap().recipient,
        Some(w1)
    );
    g.exited(w1, 0).unwrap();
    assert!(matches!(
        g.send(sender, "워커", Kind::Queue, "x"),
        Err(Refusal::Ended(_))
    ));
}

#[test]
fn directory_lists_every_node_with_its_place() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let (left, right) = two_repos(&g);
    let w = child(&g, right, "워커");
    g.exited(w, 3).unwrap();

    let rows: Vec<_> = g
        .directory()
        .unwrap()
        .into_iter()
        .map(|l| (l.id, l.name, l.parent, l.place, l.alive))
        .collect();
    assert_eq!(
        rows,
        [
            (left, "claude-1".into(), None, "repo/repo".into(), true),
            (right, "codex-1".into(), None, "other/화면".into(), true),
            (w, "워커".into(), Some(right), "other/화면".into(), false),
        ]
    );
}

#[test]
fn child_names_are_unique_among_siblings_and_parent() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    child(&g, lead, "a");

    let refused = |name| {
        g.add_child(
            lead,
            named(name),
            TreeLimits::default(),
            &launched(),
            started,
        )
    };
    assert!(matches!(refused("a"), Err(Refusal::NameTaken(_))));
    assert!(matches!(refused("총괄-1"), Err(Refusal::NameTaken(_))));
    assert!(matches!(refused("  "), Err(Refusal::EmptyName)));
}

#[test]
fn tree_limits_come_from_the_caller() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let flat = TreeLimits {
        max_depth: Some(0),
        max_children: None,
    };
    assert!(matches!(
        g.add_child(lead, named("a"), flat, &launched(), started),
        Err(Refusal::TooDeep)
    ));
    let two = TreeLimits {
        max_depth: Some(5),
        max_children: Some(2),
    };
    let a = g
        .add_child(lead, named("a"), two, &launched(), started)
        .unwrap();
    g.add_child(lead, named("b"), two, &launched(), started)
        .unwrap();
    // 끝난 자식도 센다.
    g.done(a, "끝", None, &[], None).unwrap();
    assert!(matches!(
        g.add_child(lead, named("c"), two, &launched(), started),
        Err(Refusal::TooManyChildren(2))
    ));
    // 입양도 같은 상한을 본다.
    let other = root(&g, "검토");
    assert!(matches!(
        g.adopt(lead, other, two),
        Err(Refusal::TooManyChildren(2))
    ));
    let deep = TreeLimits {
        max_depth: Some(3),
        max_children: None,
    };
    let a1 = g
        .add_child(a, named("a1"), deep, &launched(), started)
        .unwrap();
    let a2 = g
        .add_child(a1, named("a2"), deep, &launched(), started)
        .unwrap();
    // 깊이 상한이 없으면 더 깊이 만들고 데려온다.
    let open_depth = TreeLimits {
        max_depth: None,
        max_children: None,
    };
    let a3 = g
        .add_child(a2, named("a3"), open_depth, &launched(), started)
        .unwrap();
    g.add_child(a3, named("a4"), open_depth, &launched(), started)
        .unwrap();
    let loner = root(&g, "외톨이");
    g.adopt(a3, loner, open_depth).unwrap();
}

#[test]
fn tree_limits_read_null_depth_as_no_limit() {
    // 비운 칸은 null로 저장된다. 없는 값은 기본값(2)이다. 옛 설정의 숫자는 그대로다.
    let read = |json: &str| serde_json::from_str::<TreeLimits>(json).unwrap().max_depth;
    assert_eq!(read(r#"{"maxDepth":null}"#), None);
    assert_eq!(read("{}"), Some(2));
    assert_eq!(read(r#"{"maxDepth":4}"#), Some(4));
}

#[test]
fn limits_are_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let a = child(&g, lead, "a");
    let a1 = child(&g, a, "a1");

    // 중첩 깊이 2
    assert!(matches!(
        g.add_child(a1, named("a2"), TreeLimits::default(), &launched(), started),
        Err(Refusal::TooDeep)
    ));
    // 지시문 4KB
    let brief = "x".repeat(4097);
    assert!(matches!(
        g.add_child(
            lead,
            NewChild {
                brief: &brief,
                ..named("b")
            },
            TreeLimits::default(),
            &launched(),
            started
        ),
        Err(Refusal::BriefTooLong(4097))
    ));
    assert!(matches!(
        g.send(lead, "a", Kind::Queue, &brief),
        Err(Refusal::BriefTooLong(4097))
    ));
    g.send(lead, "a", Kind::Queue, &"x".repeat(4096)).unwrap();
    // done 요약 1KB
    assert!(matches!(
        g.done(a1, &"x".repeat(1025), None, &[], None),
        Err(Refusal::SummaryTooLong(1025))
    ));
    // 메일함 50: 위에서 하나를 넣었다
    for _ in 1..50 {
        g.send(lead, "a", Kind::Queue, "x").unwrap();
    }
    assert!(
        matches!(g.send(lead, "a", Kind::Queue, "x"), Err(Refusal::MailboxFull(n)) if n == "a")
    );
    take(&g, a);
    g.send(lead, "a", Kind::Queue, "x").unwrap();
}

#[test]
fn failed_launch_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let fail = |_| Err("없는 명령".to_owned());

    assert!(matches!(
        g.add_child(
            lead,
            NewChild {
                brief: "지시",
                ..named("a")
            },
            TreeLimits::default(),
            &launched(),
            fail
        ),
        Err(Refusal::Launch(_))
    ));
    assert_eq!(g.subscribe().unwrap().0.len(), 1);
    assert_eq!(take(&g, lead), None);
    let a = child(&g, lead, "a");
    assert_eq!(take(&g, a), None);
}

#[test]
fn done_sends_summary_and_later_exit_keeps_done() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let a = child(&g, lead, "a");

    g.done(a, "처음", None, &[], None).unwrap();
    g.done(a, "끝났다", None, &[], None).unwrap();
    g.exited(a, 0).unwrap();
    assert_eq!(
        view(&g, a).state,
        NodeState::Done {
            summary: "끝났다".into()
        }
    );
    assert_eq!(take(&g, lead), delivery("a", "done: 처음"));
    assert_eq!(take(&g, lead), delivery("a", "done: 끝났다"));
    assert_eq!(take(&g, lead), None);
}

#[test]
fn exit_without_done_tells_parent_even_when_mailbox_is_full() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let a = child(&g, lead, "a");
    let b = child(&g, lead, "b");
    for _ in 0..50 {
        g.send(b, "총괄-1", Kind::Queue, "x").unwrap();
    }

    g.exited(a, 3).unwrap();
    assert_eq!(view(&g, a).state, NodeState::Exited { code: Some(3) });
    assert_eq!(view(&g, lead).unread, 51);
    assert!(matches!(
        g.send(lead, "a", Kind::Queue, "x"),
        Err(Refusal::Ended(_))
    ));
}

/// 설정이 실패하면 부모는 까닭과 출력 끝을 받는다. 긴 출력은 지시문 상한 안에서 끝 쪽(오류)을 남긴다.
#[test]
fn setup_failure_tells_parent_why() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let a = child(&g, lead, "a");
    g.setup_failed(a, 2, "npm ci\nerror TS2322: 틀린 타입")
        .unwrap();
    assert_eq!(view(&g, a).state, NodeState::SetupFailed { code: 2 });
    assert_eq!(
        take(&g, lead),
        delivery(
            "a",
            "setup failed (exit 2):\nnpm ci\nerror TS2322: 틀린 타입"
        )
    );

    let b = child(&g, lead, "b");
    let long = format!("{}\n마지막 오류", "가".repeat(3000));
    g.setup_failed(b, 1, &long).unwrap();
    let (_, _, body) = take(&g, lead).unwrap();
    assert!(body.len() <= BRIEF_LIMIT_BYTES, "{}", body.len());
    assert!(body.starts_with("setup failed (exit 1):\n가"), "{body}");
    assert!(body.ends_with("\n마지막 오류"), "{body}");
}

#[test]
fn restart_keeps_nodes_and_messages() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("loom.db");
    let (lead, a) = {
        let g = open(&path);
        let lead = root(&g, "총괄");
        let a = g
            .add_child(
                lead,
                NewChild {
                    brief: "지시",
                    ..named("a")
                },
                TreeLimits::default(),
                &launched(),
                started,
            )
            .unwrap();
        g.done(a, "요약", None, &[], None).unwrap();
        (lead, a)
    };

    let g = open(&path);
    let views = g.subscribe().unwrap().0;
    assert_eq!(views.len(), 2);
    assert_eq!(view(&g, lead).state, NodeState::Running);
    assert_eq!(view(&g, a).parent, Some(lead));
    assert_eq!(
        view(&g, a).state,
        NodeState::Done {
            summary: "요약".into()
        }
    );
    assert_eq!(take(&g, a), delivery("총괄-1", "지시"));
    assert_eq!(take(&g, lead), delivery("a", "done: 요약"));
    // 다시 연 뒤 만든 노드의 id는 앞 노드와 겹치지 않는다.
    assert_eq!(root(&g, "새 노드"), NodeId(3));
}

#[tokio::test]
async fn wait_returns_when_a_message_arrives() {
    let dir = tempfile::tempdir().unwrap();
    let g = std::sync::Arc::new(open(&dir.path().join("loom.db")));
    let lead = root(&g, "총괄");
    let a = child(&g, lead, "a");

    let waiting = tokio::spawn({
        let g = g.clone();
        async move { g.wait(lead, None).await.unwrap() }
    });
    tokio::task::yield_now().await;
    assert!(!waiting.is_finished());
    g.send(a, "총괄-1", Kind::Queue, "결과").unwrap();
    let got = waiting.await.unwrap();
    assert_eq!((got.from.as_str(), got.body.as_str()), ("a", "결과"));
}

#[test]
fn an_unacknowledged_message_comes_back_in_order_even_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("loom.db");
    let g = open(&path);
    let lead = root(&g, "총괄");
    let a = child(&g, lead, "a");
    g.send(lead, "a", Kind::Queue, "1").unwrap();
    g.send(lead, "a", Kind::Queue, "2").unwrap();

    let first = g.take(a, Kind::ALL, usize::MAX).unwrap();
    assert_eq!(bodies(first.iter()), ["1", "2"]);
    // 전달 중인 메시지는 시간 상한 전에는 다른 쪽(훅, 백그라운드 `loom wait`)이 다시 꺼내지 않는다.
    assert!(g.take(a, Kind::ALL, usize::MAX).unwrap().is_empty());
    expire(&g);
    let again = g.take(a, Kind::ALL, usize::MAX).unwrap();
    assert_eq!(again, first);
    g.ack(a, &[first[0].id]).unwrap();
    // 다른 노드의 확인은 무시한다.
    g.ack(lead, &[first[1].id]).unwrap();
    expire(&g);
    assert_eq!(bodies(&g.take(a, Kind::ALL, usize::MAX).unwrap()), ["2"]);

    // 데몬이 다시 시작해도 확인하지 않은 메시지는 남는다.
    drop(g);
    let g = open(&path);
    expire(&g);
    let after = g.take(a, Kind::ALL, usize::MAX).unwrap();
    assert_eq!(
        after.iter().map(|d| d.id).collect::<Vec<_>>(),
        [first[1].id]
    );
    g.ack(a, &[first[1].id]).unwrap();
    expire(&g);
    assert!(g.take(a, Kind::ALL, usize::MAX).unwrap().is_empty());
    assert_eq!(view(&g, a).unread, 0);
}

#[test]
fn each_point_takes_only_its_kinds_in_arrival_order_and_once() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let a = child(&g, lead, "a");
    g.send(lead, "a", Kind::Queue, "q1").unwrap();
    g.send(lead, "a", Kind::Steer, "s1").unwrap();
    g.send(lead, "a", Kind::Interrupt, "i1").unwrap();
    g.send(lead, "a", Kind::Steer, "s2").unwrap();

    // PostToolUse: 조정만
    assert_eq!(
        bodies(&g.take(a, &[Kind::Steer], 8192).unwrap()),
        ["s1", "s2"]
    );
    assert!(g.take(a, &[Kind::Steer], 8192).unwrap().is_empty());
    // PreToolUse: 중단만
    let interrupt = g.take(a, &[Kind::Interrupt], 8192).unwrap();
    assert_eq!(
        interrupt
            .iter()
            .map(|d| (d.from.as_str(), d.kind, d.body.as_str()))
            .collect::<Vec<_>>(),
        [("총괄-1", Kind::Interrupt, "i1")]
    );
    // 꺼낸 메시지는 확인 전이라도 읽은 것으로 센다(2026-10-22 사람 지시 "버튼이 사라져야 하는거 아님?").
    assert_eq!(view(&g, a).unread, 1);
    g.ack(a, &[interrupt[0].id]).unwrap();
    // Stop: 남은 것 모두
    let rest = g.take(a, Kind::ALL, 8192).unwrap();
    assert_eq!(
        rest.iter().map(|d| d.body.as_str()).collect::<Vec<_>>(),
        ["q1"]
    );
    let ids: Vec<MessageId> = rest.iter().map(|d| d.id).collect();
    g.ack(a, &ids).unwrap();
    assert_eq!(view(&g, a).unread, 0);
    assert!(g.take(a, Kind::ALL, 8192).unwrap().is_empty());
}

#[test]
fn take_stops_at_the_byte_limit_but_always_takes_the_first() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let a = child(&g, lead, "a");
    for body in ["x".repeat(4096), "y".repeat(4096), "z".repeat(10)] {
        g.send(lead, "a", Kind::Queue, &body).unwrap();
    }

    assert_eq!(g.take(a, Kind::ALL, 4096).unwrap().len(), 1);
    assert_eq!(
        g.take(a, Kind::ALL, 4096).unwrap()[0].body,
        "y".repeat(4096)
    );
    assert_eq!(
        bodies(&g.take(a, Kind::ALL, 4096).unwrap()),
        ["z".repeat(10)]
    );
    g.send(lead, "a", Kind::Queue, "1").unwrap();
    g.send(lead, "a", Kind::Queue, "2").unwrap();
    assert_eq!(bodies(&g.take(a, Kind::ALL, 1).unwrap()), ["1"]);
}

#[test]
fn related_names_the_node_and_its_agent() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "claude");
    let a = g
        .add_child(
            lead,
            NewChild {
                agent: Some(AgentId("codex".into())),
                ..named("a")
            },
            TreeLimits::default(),
            &launched(),
            started,
        )
        .unwrap();

    assert_eq!(
        g.related(lead, "a").unwrap(),
        (a, Some(AgentId("codex".into())))
    );
    assert_eq!(
        g.related(a, "claude-1").unwrap(),
        (lead, Some(AgentId("claude".into())))
    );
    assert!(matches!(g.related(lead, "b"), Err(Refusal::NotRelated(_))));
}

#[test]
fn removing_a_parent_keeps_its_children_and_they_hold_unsent_messages() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let a = child(&g, lead, "a");
    let b = child(&g, a, "b");
    g.send(lead, "a", Kind::Queue, "a가 받을 지시").unwrap();
    g.send(b, "a", Kind::Queue, "질문").unwrap();
    let mut events = g.subscribe().unwrap().1;

    g.remove(a).unwrap();

    assert!(matches!(events.try_recv().unwrap(), GraphEvent::Removed { id } if id == a));
    assert!(matches!(events.try_recv().unwrap(), GraphEvent::Node(v) if v.id == b));
    let ids: Vec<NodeId> = g.subscribe().unwrap().0.iter().map(|v| v.id).collect();
    assert_eq!(ids, [lead, b]);
    // 자식은 계속 돈다. 부모가 비고, 전하지 못한 질문을 보관한다.
    let orphan = view(&g, b);
    assert_eq!((orphan.parent, orphan.state), (None, NodeState::Running));
    assert_eq!(held(&g, b), 1);
    // 실행 중이던 a를 기다리는 부모가 removed를 받는다. a가 받은 지시는 a와 함께 지운다.
    assert_eq!(take(&g, lead), delivery("a", "removed"));
    let left: u32 = g
        .lock()
        .query_row("SELECT count(*) FROM messages", [], |row| row.get(0))
        .unwrap();
    assert_eq!(left, 2);
    // 없는 노드는 아무것도 하지 않는다.
    g.remove(a).unwrap();
}

#[test]
fn removing_a_finished_node_does_not_tell_the_parent_again() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let a = child(&g, lead, "a");
    g.done(a, "요약", None, &[], None).unwrap();
    assert_eq!(take(&g, lead), delivery("a", "done: 요약"));

    g.remove(a).unwrap();
    assert_eq!(take(&g, lead), None);
    g.remove(lead).unwrap();
    assert!(g.subscribe().unwrap().0.is_empty());
}

#[test]
fn a_new_parent_gets_held_messages_in_the_order_they_were_sent() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let worker = root(&g, "조사");
    g.send(worker, "옛 총괄", Kind::Queue, "하나").unwrap();
    g.send(worker, "옛 총괄", Kind::Queue, "둘").unwrap();
    g.done(worker, "요약", Some("일해"), &[], None).unwrap();
    // 받을 노드가 정해지지 않은 조정은 보관하지 않는다.
    assert!(matches!(
        g.send(worker, "옛 총괄", Kind::Steer, "x"),
        Err(Refusal::NotRelated(_))
    ));
    // 부모 없는 완료는 받은편지함에만 간다. 보관하지 않는다.
    assert_eq!(held(&g, worker), 2);

    g.adopt(lead, worker, TreeLimits::default()).unwrap();

    let adopted = view(&g, worker);
    assert_eq!(adopted.parent, Some(lead));
    assert_eq!(held(&g, worker), 0);
    assert!(matches!(
        recorded(&g).last(),
        Some((events::Entry::Adopted { held: 2, .. }, None))
    ));
    assert_eq!(
        bodies(&g.take(lead, Kind::ALL, usize::MAX).unwrap()),
        ["하나", "둘"]
    );
    assert_eq!(
        take(&g, worker),
        delivery(
            "총괄-1",
            "parent: 총괄-1 (#1). Use loom status to inspect current relationships."
        )
    );
    // 새 부모와는 보관 없이 바로 주고받는다.
    g.send(lead, "조사-1", Kind::Queue, "다음").unwrap();
    assert_eq!(take(&g, worker), delivery("총괄-1", "다음"));
}

#[test]
fn adopting_a_running_node_tells_its_old_parent() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let other = root(&g, "검토");
    let a = child(&g, lead, "a");

    g.adopt(other, a, TreeLimits::default()).unwrap();

    assert_eq!(view(&g, a).parent, Some(other));
    assert_eq!(take(&g, lead), delivery("a", "adopted: 검토-1"));
    // 이미 그 부모면 아무것도 하지 않는다.
    g.adopt(other, a, TreeLimits::default()).unwrap();
    assert_eq!(take(&g, other), None);
}

#[test]
fn adopt_refuses_cycles_depth_and_taken_names() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let a = child(&g, lead, "a");
    let a1 = child(&g, a, "a1");
    let other = root(&g, "검토");
    child(&g, other, "b");
    // 검토 아래의 a. 총괄 아래에도 a가 있다.
    let twin = child(&g, other, "a");

    assert!(matches!(
        g.adopt(lead, lead, TreeLimits::default()),
        Err(Refusal::Cycle)
    ));
    assert!(matches!(
        g.adopt(a1, lead, TreeLimits::default()),
        Err(Refusal::Cycle)
    ));
    assert!(matches!(
        g.adopt(a, lead, TreeLimits::default()),
        Err(Refusal::Cycle)
    ));
    // a1은 깊이 2다. 자식이 있는 검토를 그 아래에 두면 깊이 4가 된다.
    assert!(matches!(
        g.adopt(a1, other, TreeLimits::default()),
        Err(Refusal::TooDeep)
    ));
    assert!(
        matches!(g.adopt(lead, twin, TreeLimits::default()), Err(Refusal::NameTaken(n)) if n == "a")
    );
    assert!(matches!(g.find("a"), Err(Refusal::Ambiguous { .. })));
    assert!(matches!(g.find("없음"), Err(Refusal::NotFound(_))));
    assert_eq!(g.find("검토-1").unwrap(), other);
    assert_eq!(view(&g, other).parent, None);
}

/// 최근 기록을 오래된 것부터, 기록과 본문만.
fn recorded(g: &Graph) -> Vec<(events::Entry, Option<String>)> {
    let mut records = g.recent_events().unwrap();
    records.reverse();
    records.into_iter().map(|r| (r.entry, r.body)).collect()
}

fn who(id: NodeId, name: &str) -> events::Who {
    events::Who {
        id: Some(id),
        name: name.into(),
    }
}

#[test]
fn records_node_and_message_events_in_order() {
    use events::{Entry, Step};
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let a = child(&g, lead, "a");
    g.send(a, "총괄-1", Kind::Queue, "질문").unwrap();
    take(&g, lead);
    g.done(a, "끝", None, &[], None).unwrap();
    let other = root(&g, "검토");
    g.adopt(other, a, TreeLimits::default()).unwrap();
    g.remove(other).unwrap();
    g.send(a, "검토-1", Kind::Queue, "보관할 말").unwrap();

    assert_eq!(
        recorded(&g),
        [
            (
                Entry::Created {
                    node: who(lead, "총괄-1"),
                    parent: None,
                    agent: Some(AgentId("총괄".into()))
                },
                None
            ),
            (
                Entry::Created {
                    node: who(a, "a"),
                    parent: Some(who(lead, "총괄-1")),
                    agent: None
                },
                None
            ),
            (
                Entry::Message {
                    how: Kind::Queue,
                    from: who(a, "a"),
                    to: Some(who(lead, "총괄-1")),
                    step: Step::Sent
                },
                Some("질문".into())
            ),
            (
                Entry::Message {
                    how: Kind::Queue,
                    from: who(a, "a"),
                    to: Some(who(lead, "총괄-1")),
                    step: Step::Delivered
                },
                Some("질문".into())
            ),
            (
                Entry::State {
                    node: who(a, "a"),
                    from: NodeState::Running,
                    to: NodeState::Done {
                        summary: "끝".into()
                    }
                },
                None
            ),
            (
                Entry::Message {
                    how: Kind::Queue,
                    from: who(a, "a"),
                    to: Some(who(lead, "총괄-1")),
                    step: Step::Sent
                },
                Some("done: 끝".into())
            ),
            (
                Entry::Created {
                    node: who(other, "검토-1"),
                    parent: None,
                    agent: Some(AgentId("검토".into()))
                },
                None
            ),
            (
                Entry::Message {
                    how: Kind::Queue,
                    from: who(other, "검토-1"),
                    to: Some(who(a, "a")),
                    step: Step::Sent,
                },
                Some(
                    "parent: 검토-1 (#3). Use loom status to inspect current relationships.".into()
                ),
            ),
            (
                Entry::Adopted {
                    node: who(a, "a"),
                    from: Some(who(lead, "총괄-1")),
                    to: who(other, "검토-1"),
                    held: 0
                },
                None
            ),
            // 지운 노드도 이름이 남는다.
            (
                Entry::Removed {
                    node: who(other, "검토-1")
                },
                None
            ),
            (
                Entry::Message {
                    how: Kind::Queue,
                    from: who(a, "a"),
                    to: None,
                    step: Step::Held
                },
                Some("보관할 말".into())
            ),
        ]
    );
}

#[test]
fn records_refusals_and_failed_launches() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    assert!(
        g.add_root(&home(), None, &launched(), |_| Err("없는 명령".into()))
            .is_err()
    );
    g.record_refusal(lead, "없음", Kind::Steer, &Refusal::NotFound("없음".into()))
        .unwrap();

    let entries: Vec<_> = recorded(&g).into_iter().map(|(e, _)| e).collect();
    assert_eq!(
        entries,
        [
            events::Entry::Created {
                node: who(lead, "총괄-1"),
                parent: None,
                agent: Some(AgentId("총괄".into()))
            },
            // 띄우지 못한 노드는 만든 뒤 지운다. 띄우는 동안 그래프를 잠그지 않으므로 줄을 먼저 커밋한다.
            events::Entry::Created {
                node: who(NodeId(lead.0 + 1), "shell-1"),
                parent: None,
                agent: None
            },
            events::Entry::Removed {
                node: who(NodeId(lead.0 + 1), "shell-1"),
            },
            events::Entry::Refused {
                how: Kind::Steer,
                from: who(lead, "총괄-1"),
                to: "없음".into(),
                reason: Refusal::NotFound("없음".into())
            },
        ]
    );
    let ids: Vec<u32> = g.recent_events().unwrap().iter().map(|r| r.id).collect();
    assert_eq!(ids, [4, 3, 2, 1]);
}

#[test]
fn screen_receives_each_record_once_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let (_, mut rx) = g.subscribe().unwrap();
    child(&g, lead, "a");
    g.send(lead, "a", Kind::Queue, "일").unwrap();

    let mut ids = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let GraphEvent::Recorded { record } = event {
            ids.push(record.id);
        }
    }
    assert_eq!(ids, [2, 3]);
}

#[test]
fn keeps_only_the_latest_records() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let entry = events::Entry::Removed {
        node: who(lead, "총괄-1"),
    };
    {
        let mut db = g.lock();
        let tx = db.transaction().unwrap();
        for _ in 0..10_000 {
            events::write(&tx, &entry, None).unwrap();
        }
        tx.commit().unwrap();
    }
    let db = g.lock();
    let (count, min): (u32, u32) = db
        .query_row("SELECT count(*), min(id) FROM events", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!((count, min), (10_000, 2));
}

#[test]
fn reopening_keeps_running_nodes_until_the_daemon_finds_them_lost() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("loom.db");
    let server = ServerLink::Codex {
        url: "ws://127.0.0.1:1".into(),
        pid: Some(9),
        thread: None,
    };
    let (lead, a) = {
        let g = open(&path);
        let lead = root(&g, "codex");
        let a = child(&g, lead, "a");
        g.set_server(lead, &server).unwrap();
        g.done(a, "요약", None, &[], None).unwrap();
        (lead, a)
    };
    let g = open(&path);
    let running = g.active().unwrap();
    assert_eq!(running.len(), 2);
    assert_eq!(running[0].id, lead);
    assert_eq!(running[0].agent, Some(AgentId("codex".into())));
    assert_eq!(running[0].server, Some(server));
    // 다시 연 데몬도 앞 데몬이 준 토큰으로 노드를 찾는다.
    assert_eq!(g.caller(&running[0].token).unwrap(), Some(lead));
    assert_eq!(g.caller(&running[1].token).unwrap(), Some(a));
    assert_eq!(g.caller("없는 토큰").unwrap(), None);
    g.lost(lead).unwrap();
    let records = recorded(&g);
    assert_eq!(
        records.last().unwrap().1.as_deref(),
        Some("exited(unknown)")
    );
    assert_eq!(
        records[records.len() - 2].0,
        events::Entry::State {
            node: who(lead, "codex-1"),
            from: NodeState::Running,
            to: NodeState::Exited { code: None }
        }
    );
    assert_eq!(
        g.active().unwrap().iter().map(|n| n.id).collect::<Vec<_>>(),
        vec![a]
    );
}

#[test]
fn roots_take_the_smallest_free_number() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    g.add_root(&home(), None, &launched(), started).unwrap();
    let first = root(&g, "claude");
    root(&g, "claude");
    // 자식 이름은 사람이 띄운 노드의 번호를 막지 않는다.
    child(&g, first, "claude-3");
    root(&g, "claude");

    let names: Vec<_> = g
        .subscribe()
        .unwrap()
        .0
        .into_iter()
        .map(|n| n.name)
        .collect();
    assert_eq!(
        names,
        ["shell-1", "claude-1", "claude-2", "claude-3", "claude-3"]
    );
}

#[test]
fn reads_adoptions_recorded_before_the_held_count() {
    let old =
        r#"{"kind":"adopted","node":{"id":2,"name":"a"},"from":null,"to":{"id":1,"name":"총괄"}}"#;
    assert!(matches!(
        serde_json::from_str(old).unwrap(),
        events::Entry::Adopted { held: 0, .. }
    ));
}

#[test]
fn children_stay_in_the_parent_place_unless_given_a_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("t.db"));
    let lead = root(&g, "claude");
    let same = child(&g, lead, "a");
    let moved = g
        .add_child(
            lead,
            NewChild {
                worktree: Some("/ws/repo/feat"),
                ..named("b")
            },
            TreeLimits::default(),
            &launched(),
            started,
        )
        .unwrap();
    assert_eq!(view(&g, same).place, home());
    assert_eq!(
        g.place_of(moved).unwrap(),
        Place {
            repo: RepoId(1),
            worktree: "/ws/repo/feat".into()
        }
    );
    assert_eq!(g.nodes_in("/repo").unwrap(), [lead, same]);
    assert_eq!(g.nodes_in("/ws/repo/feat").unwrap(), [moved]);
}

#[test]
fn settle_moves_nodes_without_a_place_to_the_start_repo() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.db");
    db::open(&path)
        .unwrap()
        .lock()
        .execute_batch(
            r#"INSERT INTO nodes (name, agent, state) VALUES ('claude-1', '"claude"', '{"kind":"running"}');"#,
        )
        .unwrap();
    let g = open(&path);
    g.settle(&home()).unwrap();
    let views = g.subscribe().unwrap().0;
    assert_eq!(views[0].place, home());
}

#[test]
fn recipient_names_cannot_collide_across_three_generations() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "lead");
    let middle = child(&g, lead, "middle");
    assert!(matches!(
        g.add_child(
            middle,
            named("lead-1"),
            TreeLimits::default(),
            &launched(),
            started
        ),
        Err(Refusal::NameTaken(_))
    ));
    let leaf = child(&g, middle, "leaf");
    // 옛 데이터에 이름 충돌이 있어도 수신자를 임의로 고르지 않는다.
    g.lock()
        .execute("UPDATE nodes SET name = 'lead-1' WHERE id = ?1", [leaf.0])
        .unwrap();
    assert!(matches!(
        g.send(middle, "lead-1", Kind::Queue, "message"),
        Err(Refusal::Ambiguous { .. })
    ));
    let sent = g
        .send(middle, &format!("#{}", leaf.0), Kind::Queue, "leaf only")
        .unwrap();
    assert_eq!(sent.recipient, Some(leaf));
    assert_eq!(take(&g, leaf), delivery("middle", "leaf only"));
    assert_eq!(take(&g, lead), None);
}

#[test]
fn adoption_checks_names_on_both_sides_of_the_new_edge() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let old = root(&g, "old");
    let middle = child(&g, old, "middle");
    let leaf = child(&g, middle, "new-1");
    let new = root(&g, "new");
    assert!(matches!(
        g.adopt(new, middle, TreeLimits::default()),
        Err(Refusal::NameTaken(_))
    ));
    assert_eq!(view(&g, middle).parent, Some(old));
    let other = child(&g, new, "other");
    assert!(matches!(
        g.adopt(other, leaf, TreeLimits::default()),
        Err(Refusal::NameTaken(_))
    ));
    assert_eq!(view(&g, leaf).parent, Some(middle));
}

#[test]
fn rejected_interrupts_do_not_stop_the_recipient() {
    use std::cell::Cell;
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "lead");
    let worker = child(&g, lead, "worker");
    let stopped = Cell::new(0);
    assert!(matches!(
        g.interrupt(lead, "worker", &"x".repeat(4097), |_| stopped
            .set(stopped.get() + 1)),
        Err(Refusal::BriefTooLong(_))
    ));
    for _ in 0..50 {
        g.send(lead, "worker", Kind::Queue, "status").unwrap();
    }
    assert!(matches!(
        g.interrupt(lead, "worker", "cancel", |_| stopped.set(stopped.get() + 1)),
        Err(Refusal::MailboxFull(_))
    ));
    assert_eq!(stopped.get(), 0);
    take(&g, worker);
    g.interrupt(lead, "worker", "cancel", |id| {
        assert_eq!(id, worker);
        stopped.set(stopped.get() + 1);
    })
    .unwrap();
    assert_eq!(stopped.get(), 1);
    g.exited(worker, 0).unwrap();
    assert!(matches!(
        g.interrupt(lead, "worker", "cancel", |_| stopped.set(stopped.get() + 1)),
        Err(Refusal::Ended(_))
    ));
    assert_eq!(stopped.get(), 1);
}

#[test]
fn completion_and_lost_notices_bypass_a_full_parent_mailbox() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "lead");
    let worker = child(&g, lead, "worker");
    let lost = child(&g, lead, "lost");
    for _ in 0..50 {
        g.send(worker, "lead-1", Kind::Queue, "status").unwrap();
    }
    g.done(worker, "finished", None, &[], None).unwrap();
    g.lost(lost).unwrap();
    g.lost(lost).unwrap();
    assert_eq!(
        view(&g, worker).state,
        NodeState::Done {
            summary: "finished".into()
        }
    );
    let messages = g.take(lead, Kind::ALL, usize::MAX).unwrap();
    assert_eq!(messages.len(), 52);
    assert_eq!(
        bodies(messages.iter().skip(50)),
        ["done: finished", "exited(unknown)"]
    );
}

#[test]
fn completed_nodes_receive_feedback_until_the_process_ends() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("loom.db");
    let g = open(&path);
    let lead = root(&g, "lead");
    let worker = child(&g, lead, "worker");
    g.done(worker, "finished", None, &[], None).unwrap();
    g.send(lead, "worker", Kind::Queue, "feedback").unwrap();
    assert_eq!(take(&g, worker), delivery("lead-1", "feedback"));
    drop(g);
    let g = open(&path);
    assert!(g.active().unwrap().iter().any(|n| n.id == worker));
    assert!(g.relations(worker).unwrap().me.alive);
    g.exited(worker, 0).unwrap();
    assert!(!g.relations(worker).unwrap().me.alive);
    assert!(!g.active().unwrap().iter().any(|n| n.id == worker));
    // 피드백을 받아 다시 일하다 끝났다. 다시 done을 부르지 않았으므로 종료다.
    assert_eq!(view(&g, worker).state, NodeState::Exited { code: Some(0) });
    assert!(matches!(
        g.send(lead, "worker", Kind::Queue, "more feedback"),
        Err(Refusal::Ended(_))
    ));
}

#[tokio::test]
async fn replies_match_the_request_even_after_another_consumer_acknowledged_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("loom.db");
    let g = open(&path);
    let lead = root(&g, "lead");
    let worker = child(&g, lead, "worker");
    let first = g
        .send(lead, "worker", Kind::Queue, "first question")
        .unwrap();
    let second = g
        .send(lead, "worker", Kind::Queue, "second question")
        .unwrap();
    assert!(matches!(
        g.reply(lead, first.id, "forged"),
        Err(Refusal::NotFound(_))
    ));
    g.reply(worker, second.id, "second answer").unwrap();
    g.reply(worker, first.id, "first answer").unwrap();
    // Stop 훅이 먼저 받아도 기다리는 질문의 답을 찾는다.
    let deliveries = g.take(lead, Kind::ALL, usize::MAX).unwrap();
    g.ack(lead, &deliveries.iter().map(|d| d.id).collect::<Vec<_>>())
        .unwrap();
    drop(g);
    let g = open(&path);
    let answer = g.wait(lead, Some(first.id)).await.unwrap();
    assert_eq!(answer.body, "first answer");
    assert_eq!(answer.reply_to, Some(first.id));
    assert_eq!(
        g.wait(lead, Some(second.id)).await.unwrap().body,
        "second answer"
    );
    assert!(matches!(
        g.wait(worker, Some(first.id)).await,
        Err(Refusal::NotFound(_))
    ));
}

#[tokio::test]
async fn waiting_for_a_reply_keeps_unrelated_mail_and_can_be_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "lead");
    let worker = child(&g, lead, "worker");
    let question = g.send(lead, "worker", Kind::Queue, "question").unwrap();
    g.send(worker, "lead-1", Kind::Queue, "unrelated").unwrap();
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(5),
            g.wait(lead, Some(question.id))
        )
        .await
        .is_err()
    );
    assert_eq!(take(&g, lead), delivery("worker", "unrelated"));
    g.reply(worker, question.id, "answer").unwrap();
    assert_eq!(
        g.wait(lead, Some(question.id)).await.unwrap().body,
        "answer"
    );
}

/// o-mode 한 바퀴: 총괄이 코디를, 코디가 워커를 띄운다. 묻고 답하고, 완료와 반려를 주고받는다.
/// 회수는 데몬 `delete_worktree`와 같이 워크트리마다 `nodes_in` 다음 `remove`를 부른다.
#[tokio::test]
async fn lead_coordinator_and_worker_spawn_talk_finish_and_are_reclaimed() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "claude");
    let spawn = |parent, name, worktree, brief| {
        g.add_child(
            parent,
            NewChild {
                name,
                worktree: Some(worktree),
                agent: Some(AgentId("claude".into())),
                brief,
            },
            TreeLimits::default(),
            &launched(),
            started,
        )
        .unwrap()
    };
    let coord = spawn(
        lead,
        "코디-1",
        "/ws/loom/1-기능",
        "Read coord.md and follow it.",
    );
    assert_eq!(
        take(&g, coord),
        delivery("claude-1", "Read coord.md and follow it.")
    );
    let worker = spawn(
        coord,
        "워커-1-화면",
        "/ws/loom/1-화면",
        "Read worker.md and follow it.",
    );
    assert_eq!(
        take(&g, worker),
        delivery("코디-1", "Read worker.md and follow it.")
    );
    assert_eq!(view(&g, worker).place.worktree, "/ws/loom/1-화면");
    // 깊이 상한 2: 워커는 자식을 만들지 못한다.
    assert!(matches!(
        g.add_child(
            worker,
            named("손자"),
            TreeLimits::default(),
            &launched(),
            started
        ),
        Err(Refusal::TooDeep)
    ));
    // 총괄도 손자에게 바로 보낸다.
    g.send(lead, "워커-1-화면", Kind::Queue, "x").unwrap();
    assert_eq!(
        take(&g, worker).map(|(_, _, body)| body).as_deref(),
        Some("x")
    );

    let question = g
        .send(worker, "코디-1", Kind::Queue, "QUESTION: 색은?")
        .unwrap();
    assert_eq!(take(&g, coord), delivery("워커-1-화면", "QUESTION: 색은?"));
    g.reply(coord, question.id, "ANSWER: 파랑").unwrap();
    let answer = g.wait(worker, Some(question.id)).await.unwrap();
    assert_eq!(
        (answer.from.as_str(), answer.body.as_str()),
        ("코디-1", "ANSWER: 파랑")
    );
    g.ack(worker, &[answer.id]).unwrap();
    g.send(coord, "claude-1", Kind::Queue, "STATUS: 워커 하나")
        .unwrap();
    assert_eq!(take(&g, lead), delivery("코디-1", "STATUS: 워커 하나"));

    g.done(worker, "1-화면 커밋", None, &[], None).unwrap();
    assert_eq!(
        take(&g, coord),
        delivery("워커-1-화면", "done: 1-화면 커밋")
    );
    // 완료한 워커도 반려를 받는다. 받으면 다시 일하는 중이고, 고친 뒤 done을 또 부른다.
    g.send(coord, "워커-1-화면", Kind::Queue, "Rework: 여백")
        .unwrap();
    assert_eq!(take(&g, worker), delivery("코디-1", "Rework: 여백"));
    assert_eq!(view(&g, worker).state, NodeState::Running);
    g.done(worker, "여백 고침", None, &[], None).unwrap();
    assert_eq!(take(&g, coord), delivery("워커-1-화면", "done: 여백 고침"));
    g.done(coord, "1-기능 병합 준비", None, &[], None).unwrap();
    assert_eq!(take(&g, lead), delivery("코디-1", "done: 1-기능 병합 준비"));
    let children = g.relations(lead).unwrap().children;
    assert_eq!(children.len(), 1);
    assert!(children[0].alive);

    let tokens: Vec<String> = g
        .active()
        .unwrap()
        .into_iter()
        .filter(|a| [coord, worker].contains(&a.id))
        .map(|a| a.token)
        .collect();
    assert_eq!(tokens.len(), 2);
    for worktree in ["/ws/loom/1-화면", "/ws/loom/1-기능"] {
        for node in g.nodes_in(worktree).unwrap() {
            g.remove(node).unwrap();
        }
    }
    // 끝난 노드를 지우면 부모에게 `removed`를 다시 보내지 않는다.
    assert_eq!(take(&g, lead), None);
    assert!(g.relations(lead).unwrap().children.is_empty());
    let ids: Vec<_> = g.subscribe().unwrap().0.iter().map(|v| v.id).collect();
    assert_eq!(ids, [lead]);
    for token in tokens {
        assert_eq!(g.caller(&token).unwrap(), None);
    }
}

#[test]
fn current_parent_is_available_to_the_child_after_adoption() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let old = root(&g, "old");
    let worker = child(&g, old, "worker");
    let new = root(&g, "new");
    g.adopt(new, worker, TreeLimits::default()).unwrap();
    let relations = g.relations(worker).unwrap();
    assert_eq!(relations.parent.unwrap().id, new);
    assert!(relations.me.alive);
    assert_eq!(g.relations(new).unwrap().children[0].id, worker);
    assert_eq!(
        take(&g, worker),
        delivery(
            "new-1",
            "parent: new-1 (#3). Use loom status to inspect current relationships."
        )
    );
}

#[test]
fn empty_shell_cleanup_preserves_relationships_messages_and_results() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let shell = g.add_root(&home(), None, &launched(), started).unwrap();
    assert!(g.discard_finished_shell(shell).unwrap());
    assert!(g.subscribe().unwrap().0.is_empty());
    assert!(!g.discard_finished_shell(shell).unwrap());

    let agent = root(&g, "codex");
    assert!(!g.discard_finished_shell(agent).unwrap());
    let nested = child(&g, agent, "nested");
    assert!(!g.discard_finished_shell(nested).unwrap());

    let parent = g.add_root(&home(), None, &launched(), started).unwrap();
    let worker = child(&g, parent, "worker");
    assert!(!g.discard_finished_shell(parent).unwrap());
    g.send(
        worker,
        &view(&g, parent).name,
        Kind::Queue,
        "review this output",
    )
    .unwrap();
    g.remove(worker).unwrap();
    assert!(!g.discard_finished_shell(parent).unwrap());
    assert_eq!(take(&g, parent), delivery("worker", "review this output"));

    let completed = g.add_root(&home(), None, &launched(), started).unwrap();
    g.done(completed, "result to preserve", Some("일해"), &[], None)
        .unwrap();
    assert!(!g.discard_finished_shell(completed).unwrap());
    assert_eq!(held(&g, completed), 0);
    assert_eq!(view(&g, completed).waits.len(), 1);

    let failed = g.add_root(&home(), None, &launched(), started).unwrap();
    g.exited(failed, 1).unwrap();
    assert!(!g.discard_finished_shell(failed).unwrap());
    assert_eq!(view(&g, failed).state, NodeState::Exited { code: Some(1) });
}

#[test]
fn rename_refuses_names_relatives_use() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "lead");
    let other_root = root(&g, "other");
    let middle = child(&g, lead, "middle");
    let sibling = child(&g, lead, "sibling");
    let leaf = child(&g, middle, "leaf");
    let deep = TreeLimits {
        max_depth: Some(3),
        ..TreeLimits::default()
    };
    g.add_child(leaf, named("tip"), deep, &launched(), started)
        .unwrap();
    let taken = |id, name| matches!(g.rename(id, name), Err(Refusal::NameTaken(n)) if n == name);
    assert!(taken(middle, "lead-1"), "parent");
    assert!(taken(leaf, "lead-1"), "grandparent");
    assert!(taken(middle, "sibling"), "sibling");
    assert!(taken(other_root, "lead-1"), "root sibling");
    assert!(taken(middle, "leaf"), "child");
    assert!(taken(middle, "tip"), "grandchild");
    assert!(matches!(g.rename(middle, "#3"), Err(Refusal::ReservedName(n)) if n == "#3"));
    assert!(matches!(g.rename(middle, " "), Err(Refusal::EmptyName)));
    // 이름으로 서로 찾지 않는 노드끼리는 같은 이름을 쓸 수 있다: 형제의 손자, 다른 나무의 노드.
    g.rename(sibling, "tip").unwrap();
    g.rename(other_root, "middle").unwrap();
    assert_eq!(view(&g, sibling).name, "tip");
    assert_eq!(view(&g, other_root).name, "middle");
}

#[test]
fn rename_records_the_old_name_and_tells_parent_and_children() {
    use events::Entry;
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "lead");
    let middle = child(&g, lead, "middle");
    let a = child(&g, middle, "a");
    let b = child(&g, middle, "b");
    g.rename(middle, "  코디 ").unwrap();
    assert_eq!(view(&g, middle).name, "코디");
    let notice = format!(
        "renamed: middle -> 코디 (#{0}). Address it as 코디 or '#{0}'.",
        middle.0
    );
    for to in [lead, a, b] {
        assert_eq!(take(&g, to), delivery("코디", &notice));
    }
    assert!(recorded(&g).contains(&(
        Entry::Renamed {
            node: who(middle, "코디"),
            from: "middle".into()
        },
        None
    )));
    // 새 이름으로 주고받는다.
    g.send(a, "코디", Kind::Queue, "hi").unwrap();
    assert_eq!(take(&g, middle), delivery("a", "hi"));
    // 같은 이름이면 아무것도 바꾸지 않는다.
    let before = recorded(&g).len();
    g.rename(middle, "코디 ").unwrap();
    assert_eq!(recorded(&g).len(), before);
    assert_eq!(take(&g, lead), None);
}

#[test]
fn a_root_that_called_done_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "lead");
    assert_eq!(g.unreported_root(lead), Ok(true));
    g.done(lead, "병합했다", Some("병합해"), &[], None).unwrap();
    assert_eq!(g.unreported_root(lead), Ok(false));
}

#[test]
fn names_for_ids_and_the_parent_are_reserved() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "lead");
    for name in ["#2", "2", "parent"] {
        assert!(matches!(
            g.add_child(
                lead,
                named(name),
                TreeLimits::default(),
                &launched(),
                started
            ),
            Err(Refusal::ReservedName(_))
        ));
    }
}

/// 에이전트는 부모를 `parent`로, 노드를 `#` 없는 번호로도 가리킨다.
#[test]
fn parent_and_bare_ids_address_nodes() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "lead");
    let a = child(&g, lead, "a");

    g.send(a, "parent", Kind::Queue, "up").unwrap();
    assert_eq!(take(&g, lead), delivery("a", "up"));
    g.send(a, "parent", Kind::Steer, "steer up").unwrap();
    assert_eq!(
        take(&g, lead),
        Some(("a".into(), Kind::Steer, "steer up".into()))
    );
    g.send(lead, &a.0.to_string(), Kind::Queue, "down").unwrap();
    assert_eq!(take(&g, a), delivery("lead-1", "down"));
    assert!(matches!(
        g.send(lead, "parent", Kind::Steer, "x"),
        Err(Refusal::NotRelated(_))
    ));
    // 부모 없는 노드가 `parent`로 보낸 큐 메시지는 다음 부모를 기다린다.
    assert_eq!(
        g.send(lead, "parent", Kind::Queue, "kept")
            .unwrap()
            .recipient,
        None
    );
    assert!(matches!(
        g.send(lead, "999", Kind::Queue, "x"),
        Err(Refusal::NotFound(_))
    ));
}

#[test]
fn a_node_does_not_enter_a_repo_outside_the_list() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("t.db"));
    let gone = Place {
        repo: RepoId(2),
        worktree: "/gone".into(),
    };
    assert_eq!(
        g.add_root(&gone, None, &launched(), |_| panic!("launched")),
        Err(Refusal::NoRepo)
    );
    assert!(g.subscribe().unwrap().0.is_empty());
}

#[test]
fn remove_child_refuses_nodes_that_are_not_my_children() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let a = child(&g, lead, "a");
    let b = child(&g, lead, "b");

    assert!(
        matches!(g.remove_child(a, "총괄-1", TreeLimits::default()), Err(Refusal::NotChild(n)) if n == "총괄-1")
    );
    assert!(matches!(
        g.remove_child(a, "b", TreeLimits::default()),
        Err(Refusal::NotRelated(_))
    ));
    assert_eq!(view(&g, b).parent, Some(lead));
    assert_eq!(view(&g, lead).parent, None);
}

#[test]
fn remove_child_moves_grandchildren_to_the_caller() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let coord = child(&g, lead, "코디");
    let worker = child(&g, coord, "워커");
    g.send(worker, "코디", Kind::Queue, "결과").unwrap();

    let (removed, moved) = g
        .remove_child(lead, &format!("#{}", coord.0), TreeLimits::default())
        .unwrap();

    assert_eq!(removed, coord);
    assert_eq!(moved, ["워커"]);
    assert!(g.find("코디").is_err());
    assert_eq!(view(&g, worker).parent, Some(lead));
    // 지운 노드에게 가던 메시지는 새 부모가 보낸 순서대로 받는다.
    assert_eq!(take(&g, lead), delivery("워커", "결과"));
    assert_eq!(take(&g, lead), delivery("코디", "removed"));
    assert_eq!(
        take(&g, worker).map(|(_, _, body)| body),
        Some(format!(
            "parent: 총괄-1 (#{}). Use loom status to inspect current relationships.",
            lead.0
        ))
    );
}

#[test]
fn remove_child_changes_nothing_when_a_grandchild_cannot_move() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let coord = child(&g, lead, "코디");
    child(&g, coord, "w1");
    let w2 = child(&g, coord, "w2");
    let one_child = TreeLimits {
        max_children: Some(1),
        ..TreeLimits::default()
    };

    let refused = g.remove_child(lead, "코디", one_child).unwrap_err();

    assert!(matches!(&refused, Refusal::ChildrenStay(list) if list.starts_with("코디: w2 (")));
    assert_eq!(view(&g, coord).parent, Some(lead));
    assert_eq!(view(&g, w2).parent, Some(coord));
    assert_eq!(take(&g, lead), None);
}

#[test]
fn done_returns_children_whose_process_still_runs() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let coord = child(&g, lead, "코디");
    let ended = child(&g, coord, "끝난워커");
    child(&g, coord, "워커");
    g.exited(ended, 0).unwrap();

    assert_eq!(g.done(coord, "요약", None, &[], None).unwrap(), ["워커"]);
}

#[test]
fn bookmarks_keep_their_order_and_vanish_with_the_node() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("loom.db");
    let g = open(&path);
    let (a, b) = (root(&g, "codex"), root(&g, "codex"));
    g.bookmark(b).unwrap();
    g.bookmark(a).unwrap();
    // 다시 북마크해도 차례가 그대로다.
    g.bookmark(b).unwrap();
    assert_eq!(
        (view(&g, a).bookmark, view(&g, b).bookmark),
        (Some(2), Some(1))
    );
    g.unbookmark(b).unwrap();
    assert_eq!(view(&g, b).bookmark, None);
    assert!(matches!(g.bookmark(NodeId(99)), Err(Refusal::NotFound(_))));
    drop(g);
    // 데몬을 다시 띄워도 남는다.
    let g = open(&path);
    assert_eq!(view(&g, a).bookmark, Some(2));
    g.remove(a).unwrap();
    assert!(
        g.subscribe()
            .unwrap()
            .0
            .iter()
            .all(|v| v.bookmark.is_none())
    );
}

#[test]
fn clicking_unread_turns_waiting_queue_mail_into_steer() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let a = child(&g, lead, "a");
    let b = child(&g, lead, "b");
    g.send(lead, "a", Kind::Queue, "q1").unwrap();
    g.send(lead, "a", Kind::Queue, "q2").unwrap();
    g.send(lead, "b", Kind::Queue, "다른 노드").unwrap();
    // 꺼낸 메시지는 읽은 것이다. 바꾸지 않는다.
    assert_eq!(g.take(a, &[Kind::Queue], 1).unwrap().len(), 1);
    assert_eq!(view(&g, a).unread, 1);

    g.steer_queued(a).unwrap();
    assert_eq!(view(&g, a).unread, 0);
    let steered = g.take(a, &[Kind::Steer], usize::MAX).unwrap();
    assert_eq!(
        steered.iter().map(|d| d.body.as_str()).collect::<Vec<_>>(),
        ["q2"]
    );
    assert_eq!(g.take(b, &[Kind::Steer], usize::MAX).unwrap().len(), 0);
    assert_eq!(g.take(b, &[Kind::Queue], usize::MAX).unwrap().len(), 1);
}

/// 확인하지 않은 완료는 사람 입력, 신호의 대기, 다시 띄우기, 프로세스 끝, 데몬 재시작으로 지워지지 않는다.
#[test]
fn unconfirmed_dones_survive_input_relaunch_exit_and_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("loom.db");
    let g = open(&path);
    let lead = root(&g, "claude");
    let other = root(&g, "codex");
    let ids = |g: &Graph, id| view(g, id).waits.iter().map(|w| w.id).collect::<Vec<_>>();
    g.done(lead, "하나", Some("일 1"), &[], None).unwrap();
    g.done(other, "다른 노드", Some("일 B"), &[], None).unwrap();
    g.done(lead, "둘", Some("일 2"), &[], None).unwrap();
    let kept = ids(&g, lead);
    assert_eq!(kept.len(), 2);
    g.human_input(lead).unwrap();
    g.typed(lead).unwrap();
    g.set_wait(lead, WaitSource::Hook, Some((WaitKind::Approval, "Bash")))
        .unwrap();
    g.set_wait(lead, WaitSource::Hook, None).unwrap();
    g.clear_wait(lead).unwrap();
    g.exited(lead, 0).unwrap();
    assert_eq!(ids(&g, lead), kept);
    drop(g);
    let g = open(&path);
    assert_eq!(ids(&g, lead), kept);
    assert_eq!(ids(&g, other).len(), 1);
}

#[test]
fn a_done_node_runs_again_when_the_human_types() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    g.done(lead, "끝", Some("일해"), &[], None).unwrap();
    // 꺼낼 메시지가 없으면 그대로 둔다.
    assert!(g.take(lead, Kind::ALL, usize::MAX).unwrap().is_empty());
    assert!(matches!(view(&g, lead).state, NodeState::Done { .. }));
    g.human_input(lead).unwrap();
    assert_eq!(view(&g, lead).state, NodeState::Running);
    g.done(lead, "또 끝", Some("또 일해"), &[], None).unwrap();
}

/// 받은편지함 알림과 사람의 응답은 길마다 알림 이벤트 한 줄을 남긴다. 신호의 대기는 남기지 않는다.
#[test]
fn records_inbox_notices_and_the_human_responses() {
    use events::{ClosedBy, Entry};
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "claude");
    let me = || who(lead, "claude-1");
    let notices = || {
        recorded(&g)
            .into_iter()
            .map(|(e, _)| e)
            .filter(|e| {
                matches!(
                    e,
                    Entry::Asked { .. }
                        | Entry::Reported { .. }
                        | Entry::Answered { .. }
                        | Entry::Instructed { .. }
                        | Entry::Unanswered { .. }
                        | Entry::Confirmed { .. }
                )
            })
            .collect::<Vec<_>>()
    };

    let q1 = g.ask(lead, "A or B?\nmore", None).unwrap();
    g.answer(q1, " A ", &[PathBuf::from("/tmp/x.png")]).unwrap();
    let q2 = g.ask(lead, "C?", None).unwrap();
    g.answered(lead, &[q2]).unwrap();
    let q3 = g.ask(lead, "D?", None).unwrap();
    g.dismiss(q3).unwrap();
    let q4 = g.ask(lead, "E?", None).unwrap();
    g.clear_wait(lead).unwrap();
    g.set_wait(lead, WaitSource::Hook, Some((WaitKind::Approval, "Bash")))
        .unwrap();
    g.dismiss(view(&g, lead).waits[0].id).unwrap();
    g.done(lead, "고쳤다.", Some("고쳐"), &[], None).unwrap();
    let d1 = view(&g, lead).waits[0].id;
    g.answer(d1, "하나 더", &[]).unwrap();
    g.done(lead, "또 고쳤다.", Some("하나 더"), &[], None)
        .unwrap();
    let d2 = view(&g, lead).waits[0].id;
    g.dismiss(d2).unwrap();
    let q5 = g.ask(lead, "F?", None).unwrap();
    g.exited(lead, 0).unwrap();

    assert_eq!(
        notices(),
        [
            Entry::Asked {
                node: me(),
                wait: q1,
                text: "A or B?\nmore".into()
            },
            Entry::Answered {
                node: me(),
                wait: q1,
                text: "A".into(),
                images: 1
            },
            Entry::Asked {
                node: me(),
                wait: q2,
                text: "C?".into()
            },
            Entry::Unanswered {
                node: me(),
                wait: q2,
                by: ClosedBy::Agent
            },
            Entry::Asked {
                node: me(),
                wait: q3,
                text: "D?".into()
            },
            Entry::Unanswered {
                node: me(),
                wait: q3,
                by: ClosedBy::Human
            },
            Entry::Asked {
                node: me(),
                wait: q4,
                text: "E?".into()
            },
            Entry::Unanswered {
                node: me(),
                wait: q4,
                by: ClosedBy::Restart
            },
            Entry::Reported {
                node: me(),
                wait: d1,
                summary: "고쳤다.".into(),
                instruction: "고쳐".into()
            },
            Entry::Instructed {
                node: me(),
                wait: d1,
                text: "하나 더".into(),
                images: 0
            },
            Entry::Reported {
                node: me(),
                wait: d2,
                summary: "또 고쳤다.".into(),
                instruction: "하나 더".into()
            },
            Entry::Confirmed {
                node: me(),
                wait: d2
            },
            Entry::Asked {
                node: me(),
                wait: q5,
                text: "F?".into()
            },
            Entry::Unanswered {
                node: me(),
                wait: q5,
                by: ClosedBy::Exit
            },
        ]
    );
    // 사람의 답·추가 지시·닫힘은 알림 이벤트 한 줄이다. 같은 일을 사람이 보낸 메시지 이벤트로 또 쓰지 않는다.
    assert!(
        !recorded(&g)
            .iter()
            .any(|(e, _)| matches!(e, Entry::Message { from, .. } if from.id.is_none())),
        "no human message entry beside the notice entry"
    );
}

#[test]
fn relaunching_an_ended_node_brings_it_back_running_and_keeps_done() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "claude");
    let worker = child(&g, lead, "w");
    assert!(g.ended(lead).unwrap().is_none());

    g.exited(lead, 1).unwrap();
    assert_eq!(g.ended(lead).unwrap().map(|n| n.id), Some(lead));
    g.relaunched(lead, &launched()).unwrap();
    assert_eq!(view(&g, lead).state, NodeState::Running);
    assert!(g.ended(lead).unwrap().is_none());
    assert!(g.active().unwrap().iter().any(|n| n.id == lead));

    g.done(worker, "결과", None, &[], None).unwrap();
    g.exited(worker, 0).unwrap();
    g.relaunched(worker, &launched()).unwrap();
    assert!(matches!(view(&g, worker).state, NodeState::Done { .. }));
    assert!(g.ended(worker).unwrap().is_none());
}
