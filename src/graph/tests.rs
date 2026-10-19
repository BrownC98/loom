use std::path::Path;

use super::*;
use crate::db;

fn open(path: &Path) -> Graph {
    Graph::open(db::open(path).unwrap()).unwrap()
}

fn launched(_: NodeId) -> Result<(), String> {
    Ok(())
}

fn root(g: &Graph, name: &str) -> NodeId {
    g.add_root(name, None, launched).unwrap()
}

fn child(g: &Graph, parent: NodeId, name: &str) -> NodeId {
    g.add_child(parent, name, None, "", launched).unwrap()
}

fn take(g: &Graph, node: NodeId) -> Option<Delivery> {
    g.take(node, Kind::ALL, 0).unwrap().pop()
}

fn delivery(from: &str, body: &str) -> Option<Delivery> {
    Some(Delivery {
        from: from.into(),
        kind: Kind::Queue,
        body: body.into(),
    })
}

fn bodies(taken: Vec<Delivery>) -> Vec<String> {
    taken.into_iter().map(|d| d.body).collect()
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
fn parent_and_child_exchange_messages() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let worker = g
        .add_child(lead, "구현", None, "node_host를 고쳐라", launched)
        .unwrap();

    assert_eq!(take(&g, worker), delivery("총괄", "node_host를 고쳐라"));
    g.send(worker, "총괄", Kind::Queue, "질문 하나").unwrap();
    assert_eq!(view(&g, lead).unread, 1);
    assert_eq!(take(&g, lead), delivery("구현", "질문 하나"));
    assert_eq!(view(&g, lead).unread, 0);
    g.send(lead, "구현", Kind::Queue, "답").unwrap();
    assert_eq!(take(&g, worker), delivery("총괄", "답"));
    assert_eq!(take(&g, worker), None);
}

#[test]
fn messages_go_only_between_parent_and_child() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let a = child(&g, lead, "a");
    child(&g, lead, "b");
    let a1 = child(&g, a, "a1");

    assert!(matches!(g.send(a, "b", Kind::Queue, "x"), Err(Refusal::NotRelated(n)) if n == "b"));
    assert!(matches!(
        g.send(lead, "a1", Kind::Queue, "x"),
        Err(Refusal::NotRelated(_))
    ));
    assert!(matches!(
        g.send(a1, "총괄", Kind::Queue, "x"),
        Err(Refusal::NotRelated(_))
    ));
    g.send(a1, "a", Kind::Queue, "x").unwrap();
}

#[test]
fn child_names_are_unique_among_siblings_and_parent() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    child(&g, lead, "a");

    let refused = |name| g.add_child(lead, name, None, "", launched);
    assert!(matches!(refused("a"), Err(Refusal::NameTaken(_))));
    assert!(matches!(refused("총괄"), Err(Refusal::NameTaken(_))));
    assert!(matches!(refused("  "), Err(Refusal::EmptyName)));
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
        g.add_child(a1, "a2", None, "", launched),
        Err(Refusal::TooDeep)
    ));
    // 지시문 4KB
    let brief = "x".repeat(4097);
    assert!(matches!(
        g.add_child(lead, "b", None, &brief, launched),
        Err(Refusal::BriefTooLong(4097))
    ));
    assert!(matches!(
        g.send(lead, "a", Kind::Queue, &brief),
        Err(Refusal::BriefTooLong(4097))
    ));
    g.send(lead, "a", Kind::Queue, &"x".repeat(4096)).unwrap();
    // done 요약 1KB
    assert!(matches!(
        g.done(a1, &"x".repeat(1025)),
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
        g.add_child(lead, "a", None, "지시", fail),
        Err(Refusal::Launch(_))
    ));
    assert_eq!(g.subscribe().unwrap().0.len(), 1);
    child(&g, lead, "a");
}

#[test]
fn done_sends_summary_and_later_exit_keeps_done() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let a = child(&g, lead, "a");

    g.done(a, "끝났다").unwrap();
    assert!(matches!(g.done(a, "다시"), Err(Refusal::AlreadyDone)));
    g.exited(a, 0).unwrap();
    assert_eq!(
        view(&g, a).state,
        NodeState::Done {
            summary: "끝났다".into()
        }
    );
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
        g.send(b, "총괄", Kind::Queue, "x").unwrap();
    }

    g.exited(a, 3).unwrap();
    assert_eq!(view(&g, a).state, NodeState::Exited { code: Some(3) });
    assert_eq!(view(&g, lead).unread, 51);
    assert!(matches!(
        g.send(lead, "a", Kind::Queue, "x"),
        Err(Refusal::Ended(_))
    ));
}

#[test]
fn restart_keeps_nodes_and_messages() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("loom.db");
    let (lead, a) = {
        let g = open(&path);
        let lead = root(&g, "총괄");
        let a = g.add_child(lead, "a", None, "지시", launched).unwrap();
        g.done(a, "요약").unwrap();
        (lead, a)
    };

    let g = open(&path);
    let views = g.subscribe().unwrap().0;
    assert_eq!(views.len(), 2);
    assert_eq!(view(&g, lead).state, NodeState::Exited { code: None });
    assert_eq!(view(&g, a).parent, Some(lead));
    assert_eq!(
        view(&g, a).state,
        NodeState::Done {
            summary: "요약".into()
        }
    );
    assert_eq!(take(&g, a), delivery("총괄", "지시"));
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
        async move { g.wait(lead).await.unwrap() }
    });
    tokio::task::yield_now().await;
    assert!(!waiting.is_finished());
    g.send(a, "총괄", Kind::Queue, "결과").unwrap();
    assert_eq!(Some(waiting.await.unwrap()), delivery("a", "결과"));
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
        bodies(g.take(a, &[Kind::Steer], 8192).unwrap()),
        ["s1", "s2"]
    );
    assert!(g.take(a, &[Kind::Steer], 8192).unwrap().is_empty());
    // PreToolUse: 중단만
    let interrupt = g.take(a, &[Kind::Interrupt], 8192).unwrap();
    assert_eq!(
        interrupt,
        [Delivery {
            from: "총괄".into(),
            kind: Kind::Interrupt,
            body: "i1".into()
        }]
    );
    assert_eq!(view(&g, a).unread, 1);
    // Stop: 남은 것 모두
    assert_eq!(bodies(g.take(a, Kind::ALL, 8192).unwrap()), ["q1"]);
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
        bodies(g.take(a, Kind::ALL, 4096).unwrap()),
        ["z".repeat(10)]
    );
    g.send(lead, "a", Kind::Queue, "1").unwrap();
    g.send(lead, "a", Kind::Queue, "2").unwrap();
    assert_eq!(bodies(g.take(a, Kind::ALL, 1).unwrap()), ["1"]);
}

#[test]
fn recipient_names_the_node_and_its_agent() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = g
        .add_root("총괄", Some(AgentId("claude".into())), launched)
        .unwrap();
    let a = child(&g, lead, "a");

    let (id, agent) = g.recipient(a, "총괄").unwrap();
    assert_eq!((id, agent), (lead, Some(AgentId("claude".into()))));
    assert_eq!(g.recipient(lead, "a").unwrap(), (a, None));
    assert!(matches!(
        g.recipient(lead, "b"),
        Err(Refusal::NotRelated(_))
    ));
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
    assert_eq!(
        (orphan.parent, orphan.state, orphan.held),
        (None, NodeState::Running, 1)
    );
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
    g.done(a, "요약").unwrap();
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
    g.done(worker, "요약").unwrap();
    // 받을 노드가 정해지지 않은 조정은 보관하지 않는다.
    assert!(matches!(
        g.send(worker, "옛 총괄", Kind::Steer, "x"),
        Err(Refusal::NotRelated(_))
    ));
    assert_eq!(view(&g, worker).held, 3);

    g.adopt(lead, worker).unwrap();

    let adopted = view(&g, worker);
    assert_eq!((adopted.parent, adopted.held), (Some(lead), 0));
    assert_eq!(
        bodies(g.take(lead, Kind::ALL, usize::MAX).unwrap()),
        ["하나", "둘", "done: 요약"]
    );
    // 새 부모와는 보관 없이 바로 주고받는다.
    g.send(lead, "조사", Kind::Queue, "다음").unwrap();
    assert_eq!(take(&g, worker), delivery("총괄", "다음"));
}

#[test]
fn adopting_a_running_node_tells_its_old_parent() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    let other = root(&g, "검토");
    let a = child(&g, lead, "a");

    g.adopt(other, a).unwrap();

    assert_eq!(view(&g, a).parent, Some(other));
    assert_eq!(take(&g, lead), delivery("a", "adopted: 검토"));
    // 이미 그 부모면 아무것도 하지 않는다.
    g.adopt(other, a).unwrap();
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
    let twin = root(&g, "a");

    assert!(matches!(g.adopt(lead, lead), Err(Refusal::Cycle)));
    assert!(matches!(g.adopt(a1, lead), Err(Refusal::Cycle)));
    assert!(matches!(g.adopt(a, lead), Err(Refusal::Cycle)));
    // a1은 깊이 2다. 자식이 있는 검토를 그 아래에 두면 깊이 4가 된다.
    assert!(matches!(g.adopt(a1, other), Err(Refusal::TooDeep)));
    assert!(matches!(g.adopt(lead, twin), Err(Refusal::NameTaken(n)) if n == "a"));
    assert!(matches!(g.find("a"), Err(Refusal::Ambiguous(_))));
    assert!(matches!(g.find("없음"), Err(Refusal::NotFound(_))));
    assert_eq!(g.find("검토").unwrap(), other);
    assert_eq!(view(&g, other).parent, None);
}
