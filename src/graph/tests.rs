use std::path::Path;

use super::*;
use crate::db;
use crate::worktree::RepoId;

fn open(path: &Path) -> Graph {
    Graph::open(db::open(path).unwrap()).unwrap()
}

/// 테스트 노드가 도는 자리.
fn home() -> Place {
    Place {
        repo: RepoId(1),
        worktree: "/repo".into(),
    }
}

fn launched(_: NodeId) -> Result<(), String> {
    Ok(())
}

/// 에이전트 id가 `agent`인 루트. 이름은 `<agent>-1`이다.
fn root(g: &Graph, agent: &str) -> NodeId {
    g.add_root(&home(), Some(AgentId(agent.into())), launched)
        .unwrap()
}

fn child(g: &Graph, parent: NodeId, name: &str) -> NodeId {
    g.add_child(parent, name, None, None, "", launched).unwrap()
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
        .add_child(lead, "구현", None, None, "node_host를 고쳐라", launched)
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
        g.send(a1, "총괄-1", Kind::Queue, "x"),
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

    let refused = |name| g.add_child(lead, name, None, None, "", launched);
    assert!(matches!(refused("a"), Err(Refusal::NameTaken(_))));
    assert!(matches!(refused("총괄-1"), Err(Refusal::NameTaken(_))));
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
        g.add_child(a1, "a2", None, None, "", launched),
        Err(Refusal::TooDeep)
    ));
    // 지시문 4KB
    let brief = "x".repeat(4097);
    assert!(matches!(
        g.add_child(lead, "b", None, None, &brief, launched),
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
        g.add_child(lead, "a", None, None, "지시", fail),
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

#[test]
fn restart_keeps_nodes_and_messages() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("loom.db");
    let (lead, a) = {
        let g = open(&path);
        let lead = root(&g, "총괄");
        let a = g
            .add_child(lead, "a", None, None, "지시", launched)
            .unwrap();
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
        async move { g.wait(lead).await.unwrap() }
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
    // 확인하기 전에는 읽지 않은 메시지로 센다.
    assert_eq!(view(&g, a).unread, 4);
    g.ack(a, &[interrupt[0].id]).unwrap();
    // Stop: 남은 것 모두
    let rest = g.take(a, Kind::ALL, 8192).unwrap();
    assert_eq!(
        rest.iter().map(|d| d.body.as_str()).collect::<Vec<_>>(),
        ["q1"]
    );
    let ids: Vec<MessageId> = rest.iter().map(|d| d.id).collect();
    g.ack(a, &ids).unwrap();
    // 조정 두 개는 확인하지 않았으므로 아직 센다.
    assert_eq!(view(&g, a).unread, 2);
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
fn recipient_names_the_node_and_its_agent() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "claude");
    let a = child(&g, lead, "a");

    let (id, agent) = g.recipient(a, "claude-1").unwrap();
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
    assert!(matches!(
        recorded(&g).last(),
        Some((events::Entry::Adopted { held: 3, .. }, None))
    ));
    assert_eq!(
        bodies(&g.take(lead, Kind::ALL, usize::MAX).unwrap()),
        ["하나", "둘", "done: 요약"]
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

    g.adopt(other, a).unwrap();

    assert_eq!(view(&g, a).parent, Some(other));
    assert_eq!(take(&g, lead), delivery("a", "adopted: 검토-1"));
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
    // 검토 아래의 a. 총괄 아래에도 a가 있다.
    let twin = child(&g, other, "a");

    assert!(matches!(g.adopt(lead, lead), Err(Refusal::Cycle)));
    assert!(matches!(g.adopt(a1, lead), Err(Refusal::Cycle)));
    assert!(matches!(g.adopt(a, lead), Err(Refusal::Cycle)));
    // a1은 깊이 2다. 자식이 있는 검토를 그 아래에 두면 깊이 4가 된다.
    assert!(matches!(g.adopt(a1, other), Err(Refusal::TooDeep)));
    assert!(matches!(g.adopt(lead, twin), Err(Refusal::NameTaken(n)) if n == "a"));
    assert!(matches!(g.find("a"), Err(Refusal::Ambiguous(_))));
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
    g.done(a, "끝").unwrap();
    let other = root(&g, "검토");
    g.adopt(other, a).unwrap();
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
fn records_refusals_and_skips_rolled_back_changes() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "총괄");
    assert!(
        g.add_root(&home(), None, |_| Err("없는 명령".into()))
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
            events::Entry::Refused {
                how: Kind::Steer,
                from: who(lead, "총괄-1"),
                to: "없음".into(),
                reason: Refusal::NotFound("없음".into())
            },
        ]
    );
    // 되돌린 노드의 기록이 없으므로 번호가 비지 않는다.
    let ids: Vec<u32> = g.recent_events().unwrap().iter().map(|r| r.id).collect();
    assert_eq!(ids, [2, 1]);
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
fn reopening_records_running_nodes_as_exited() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("loom.db");
    let lead = root(&open(&path), "총괄");
    let g = open(&path);
    let (entry, _) = recorded(&g).pop().unwrap();
    assert_eq!(
        entry,
        events::Entry::State {
            node: who(lead, "총괄-1"),
            from: NodeState::Running,
            to: NodeState::Exited { code: None }
        }
    );
}

#[test]
fn roots_take_the_smallest_free_number() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    g.add_root(&home(), None, launched).unwrap();
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
        .add_child(lead, "b", Some("/ws/repo/feat"), None, "", launched)
        .unwrap();
    assert_eq!(view(&g, same).place, home());
    assert_eq!(
        g.place_of(moved).unwrap(),
        Place {
            repo: RepoId(1),
            worktree: "/ws/repo/feat".into()
        }
    );
    assert_eq!(g.running_in("/ws/repo/feat").unwrap(), 1);
    assert_eq!(g.nodes_in("/repo").unwrap(), [lead, same]);
    g.done(moved, "끝").unwrap();
    assert_eq!(g.running_in("/ws/repo/feat").unwrap(), 0);
}

#[test]
fn settle_moves_nodes_without_a_place_to_the_start_repo() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.db");
    db::open(&path)
        .unwrap()
        .execute_batch(
            r#"INSERT INTO nodes (name, agent, state) VALUES ('claude-1', '"claude"', '{"kind":"running"}');"#,
        )
        .unwrap();
    let g = open(&path);
    g.settle(&home()).unwrap();
    let views = g.subscribe().unwrap().0;
    assert_eq!(views[0].place, home());
}
