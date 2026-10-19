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
    g.take(node).unwrap()
}

fn delivery(from: &str, body: &str) -> Option<Delivery> {
    Some(Delivery {
        from: from.into(),
        body: body.into(),
    })
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
    g.send(worker, "총괄", "질문 하나").unwrap();
    assert_eq!(view(&g, lead).unread, 1);
    assert_eq!(take(&g, lead), delivery("구현", "질문 하나"));
    assert_eq!(view(&g, lead).unread, 0);
    g.send(lead, "구현", "답").unwrap();
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

    assert!(matches!(g.send(a, "b", "x"), Err(Refusal::NotRelated(n)) if n == "b"));
    assert!(matches!(
        g.send(lead, "a1", "x"),
        Err(Refusal::NotRelated(_))
    ));
    assert!(matches!(
        g.send(a1, "총괄", "x"),
        Err(Refusal::NotRelated(_))
    ));
    g.send(a1, "a", "x").unwrap();
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
        g.send(lead, "a", &brief),
        Err(Refusal::BriefTooLong(4097))
    ));
    g.send(lead, "a", &"x".repeat(4096)).unwrap();
    // done 요약 1KB
    assert!(matches!(
        g.done(a1, &"x".repeat(1025)),
        Err(Refusal::SummaryTooLong(1025))
    ));
    // 메일함 50: 위에서 하나를 넣었다
    for _ in 1..50 {
        g.send(lead, "a", "x").unwrap();
    }
    assert!(matches!(g.send(lead, "a", "x"), Err(Refusal::MailboxFull(n)) if n == "a"));
    take(&g, a);
    g.send(lead, "a", "x").unwrap();
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
        g.send(b, "총괄", "x").unwrap();
    }

    g.exited(a, 3).unwrap();
    assert_eq!(view(&g, a).state, NodeState::Exited { code: Some(3) });
    assert_eq!(view(&g, lead).unread, 51);
    assert!(matches!(g.send(lead, "a", "x"), Err(Refusal::Ended(_))));
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
    g.send(a, "총괄", "결과").unwrap();
    assert_eq!(Some(waiting.await.unwrap()), delivery("a", "결과"));
}
