use super::*;
use crate::graph::events::Step;

fn agent_entries(g: &Graph, of: NodeId) -> Vec<Entry> {
    let mut entries: Vec<Entry> = g
        .recent_events()
        .unwrap()
        .into_iter()
        .map(|r| r.entry)
        .filter(|e| match e {
            Entry::AgentStateChanged { node, .. } | Entry::AgentStateDisagreed { node, .. } => {
                node.id == Some(of)
            }
            _ => false,
        })
        .collect();
    entries.reverse();
    entries
}

fn changes(g: &Graph, of: NodeId) -> Vec<(AgentState, AgentEvent)> {
    agent_entries(g, of)
        .into_iter()
        .filter_map(|e| match e {
            Entry::AgentStateChanged { to, by, .. } => Some((to, by)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_agent_state_survives_a_daemon_restart_and_shows_in_status() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("loom.db");
    let g = open(&path);
    let lead = root(&g, "claude");
    assert_eq!(g.agent_state(lead).unwrap(), Some(AgentState::Starting));
    g.agent_event(lead, AgentEvent::TurnStarted).unwrap();
    g.agent_event(lead, AgentEvent::TurnStarted).unwrap();
    drop(g);

    let g = open(&path);
    assert_eq!(g.agent_state(lead).unwrap(), Some(AgentState::Turn));
    assert_eq!(
        g.relations(lead).unwrap().me.agent_state,
        Some(AgentState::Turn)
    );
    assert_eq!(
        changes(&g, lead),
        [(AgentState::Turn, AgentEvent::TurnStarted)],
        "an event that keeps the state leaves no entry"
    );
    assert!(
        g.event_log()
            .unwrap()
            .records
            .iter()
            .all(|r| !matches!(r.entry, Entry::AgentStateChanged { .. })),
        "the event console does not show agent states"
    );
}

#[test]
fn exit_sleep_and_relaunch_move_the_agent_state() {
    let relaunching = || AgentState::Relaunching {
        before: Box::new(AgentState::Starting),
    };
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "claude");
    let worker = child(&g, lead, "worker");
    g.agent_event(worker, AgentEvent::WaitOpened).unwrap();
    g.done(worker, &report("finished"), None, &[], None, None)
        .unwrap();
    assert!(g.slept(worker).unwrap());
    assert_eq!(g.agent_state(worker).unwrap(), Some(AgentState::Asleep));
    g.relaunched(worker, &launched()).unwrap();
    assert_eq!(g.agent_state(worker).unwrap(), Some(AgentState::Starting));
    g.agent_event(worker, AgentEvent::RelaunchStarted).unwrap();
    g.agent_event(worker, AgentEvent::RelaunchFailed).unwrap();
    g.agent_event(worker, AgentEvent::RelaunchStarted).unwrap();
    g.lost(worker).unwrap();
    g.sleep_lost(worker).unwrap();
    assert_eq!(
        changes(&g, worker),
        [
            (AgentState::Listening, AgentEvent::WaitOpened),
            (AgentState::Asleep, AgentEvent::Slept),
            (AgentState::Starting, AgentEvent::Relaunched),
            (relaunching(), AgentEvent::RelaunchStarted),
            (AgentState::Starting, AgentEvent::RelaunchFailed),
            (relaunching(), AgentEvent::RelaunchStarted),
            (AgentState::Ended, AgentEvent::Exited),
        ]
    );
    g.exited(lead, 0).unwrap();
    assert_eq!(g.agent_state(lead).unwrap(), Some(AgentState::Ended));
}

#[test]
fn a_lifecycle_event_logs_the_change_of_a_node_without_a_stored_state() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "claude");
    let worker = child(&g, lead, "worker");
    g.done(worker, &report("finished"), None, &[], None, None)
        .unwrap();
    assert!(g.slept(worker).unwrap());
    g.exited(lead, 0).unwrap();
    assert_eq!(
        changes(&g, worker),
        [(AgentState::Asleep, AgentEvent::Slept)]
    );
    assert_eq!(changes(&g, lead), [(AgentState::Ended, AgentEvent::Exited)]);
    g.lock().execute("DELETE FROM agent_states", []).unwrap();
    g.relaunched(lead, &launched()).unwrap();
    g.sleep_lost(worker).unwrap();
    assert_eq!(
        changes(&g, lead).last(),
        Some(&(AgentState::Starting, AgentEvent::Relaunched))
    );
    assert_eq!(
        changes(&g, worker).last(),
        Some(&(AgentState::Ended, AgentEvent::Exited))
    );
}

#[test]
fn a_node_without_a_stored_state_takes_it_from_its_process() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "claude");
    let worker = child(&g, lead, "worker");
    let ended = child(&g, lead, "ended");
    g.done(worker, &report("finished"), None, &[], None, None)
        .unwrap();
    g.slept(worker).unwrap();
    g.exited(ended, 0).unwrap();
    g.lock().execute("DELETE FROM agent_states", []).unwrap();
    assert_eq!(g.agent_state(lead).unwrap(), Some(AgentState::Starting));
    assert_eq!(g.agent_state(worker).unwrap(), Some(AgentState::Asleep));
    assert_eq!(g.agent_state(ended).unwrap(), Some(AgentState::Ended));
    g.remove(ended).unwrap();
    assert_eq!(g.agent_state(ended).unwrap(), None);
    g.agent_event(ended, AgentEvent::TurnStarted).unwrap();
}

#[test]
fn a_dialog_blocks_the_agent_until_it_closes() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "claude");
    g.agent_event(lead, AgentEvent::TurnStarted).unwrap();
    let approval = (WaitKind::Approval, "Run it?");
    g.set_wait(&token(&g, lead), WaitSource::Screen, Some(approval))
        .unwrap();
    g.set_wait(&token(&g, lead), WaitSource::Screen, Some(approval))
        .unwrap();
    let blocked = AgentState::Blocked {
        before: Box::new(AgentState::Turn),
    };
    assert_eq!(g.agent_state(lead).unwrap(), Some(blocked.clone()));
    g.set_wait(&token(&g, lead), WaitSource::Screen, None)
        .unwrap();
    g.set_wait(&token(&g, lead), WaitSource::Screen, Some(approval))
        .unwrap();
    g.typed(lead).unwrap();
    g.set_wait(&token(&g, lead), WaitSource::Screen, Some(approval))
        .unwrap();
    g.dismiss(view(&g, lead).waits[0].id).unwrap();
    let cleared = (AgentState::Turn, AgentEvent::DialogCleared);
    assert_eq!(
        changes(&g, lead),
        [
            (AgentState::Turn, AgentEvent::TurnStarted),
            (blocked.clone(), AgentEvent::Dialog),
            cleared.clone(),
            (blocked.clone(), AgentEvent::Dialog),
            cleared.clone(),
            (blocked, AgentEvent::Dialog),
            cleared,
        ],
        "a dialog closes by its signal, by typing and by dismissing"
    );
}

fn agent_child(g: &Graph, parent: NodeId, name: &str) -> NodeId {
    let child = NewChild {
        agent: Some(AgentId("claude".into())),
        ..named(name)
    };
    g.add_child(parent, child, TreeLimits::default(), &launched(), started)
        .unwrap()
}

fn mail_state(g: &Graph, id: MessageId) -> (bool, bool, bool) {
    g.lock()
        .query_row(
            "SELECT sent_at IS NOT NULL, printed, delivered FROM messages WHERE id = ?1",
            [id.0],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap()
}

fn delivered_entries(g: &Graph, to: NodeId) -> usize {
    g.recent_events()
        .unwrap()
        .into_iter()
        .filter(|r| {
            matches!(&r.entry, Entry::Message { step: Step::Delivered, to: Some(who), .. } if who.id == Some(to))
        })
        .count()
}

#[test]
fn a_printed_message_is_read_at_the_next_agent_event_and_comes_back_when_the_process_goes() {
    const QUEUED: (bool, bool, bool) = (false, false, false);
    const HANDED: (bool, bool, bool) = (true, false, false);
    const PRINTED: (bool, bool, bool) = (true, true, false);
    const READ: (bool, bool, bool) = (true, true, true);
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "claude");
    let worker = agent_child(&g, lead, "worker");
    let send = |body: &str| g.note(lead, "worker", Kind::Queue, body, None).unwrap().id;
    let take = || -> Vec<MessageId> {
        g.take(worker, Kind::ALL, usize::MAX)
            .unwrap()
            .into_iter()
            .map(|d| d.id)
            .collect()
    };

    let first = send("1");
    assert_eq!(take(), [first]);
    g.ack(worker, &[first]).unwrap();
    assert_eq!(mail_state(&g, first), PRINTED);
    assert_eq!(view(&g, worker).unread, 0);
    assert_eq!(delivered_entries(&g, worker), 0, "printed is not read");
    g.agent_event(worker, AgentEvent::TurnStarted).unwrap();
    assert_eq!(mail_state(&g, first), READ);
    assert_eq!(delivered_entries(&g, worker), 1);

    for gone in [
        AgentEvent::RelaunchStarted,
        AgentEvent::Exited,
        AgentEvent::Slept,
    ] {
        let second = send("2");
        assert_eq!(take(), [second]);
        g.ack(worker, &[second]).unwrap();
        g.agent_event(worker, gone.clone()).unwrap();
        assert_eq!(mail_state(&g, second), QUEUED, "{gone:?}");
        g.ack(worker, &[second]).unwrap();
        assert_eq!(mail_state(&g, second), QUEUED, "a late ack of {gone:?}");
        g.agent_event(worker, AgentEvent::Relaunched).unwrap();
        assert_eq!(take(), [second], "the new process takes it");
        g.ack(worker, &[second]).unwrap();
        g.agent_event(worker, AgentEvent::ServerIdle).unwrap();
        assert_eq!(mail_state(&g, second), READ);
    }

    let lost = send("3");
    assert_eq!(take(), [lost]);
    g.agent_event(worker, AgentEvent::WaitClosed { by: IdleEnd::Gone })
        .unwrap();
    assert_eq!(
        mail_state(&g, lost),
        QUEUED,
        "a hook that went before printing"
    );
    assert_eq!(take(), [lost]);
    let printed = send("4");
    assert_eq!(take(), [printed]);
    g.ack(worker, &[printed]).unwrap();
    g.agent_event(worker, AgentEvent::WaitClosed { by: IdleEnd::Error })
        .unwrap();
    assert_eq!(mail_state(&g, lost), QUEUED);
    assert_eq!(mail_state(&g, printed), PRINTED, "the hook printed it");
    assert_eq!(take(), [lost]);
    g.agent_event(worker, AgentEvent::TurnEndedWaitOpened)
        .unwrap();
    assert_eq!(mail_state(&g, printed), READ);
    assert_eq!(
        mail_state(&g, lost),
        QUEUED,
        "a turn end gives back what no hook printed"
    );
    assert_eq!(take(), [lost]);
    assert_eq!(mail_state(&g, lost), HANDED);
    g.agent_event(worker, AgentEvent::WaitClosed { by: IdleEnd::Mail })
        .unwrap();
    g.agent_event(worker, AgentEvent::WaitOpened).unwrap();
    assert_eq!(
        mail_state(&g, lost),
        QUEUED,
        "a new idle wait gives back what the lost idle reply carried"
    );
    assert_eq!(take(), [lost]);
}

#[test]
fn a_handed_message_comes_back_when_the_daemon_starts_again_and_an_unlinked_node_reads_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("loom.db");
    let g = open(&path);
    let lead = root(&g, "claude");
    let worker = agent_child(&g, lead, "worker");
    let shell = child(&g, lead, "shell");
    let handed = g.note(lead, "worker", Kind::Queue, "1", None).unwrap().id;
    let printed = g.note(lead, "worker", Kind::Queue, "2", None).unwrap().id;
    let note = g.note(lead, "shell", Kind::Queue, "3", None).unwrap().id;
    assert_eq!(g.take(worker, Kind::ALL, usize::MAX).unwrap().len(), 2);
    g.ack(worker, &[printed]).unwrap();
    assert_eq!(g.take(shell, Kind::ALL, usize::MAX).unwrap().len(), 1);
    g.ack(shell, &[note]).unwrap();
    assert_eq!(mail_state(&g, note), (true, true, true));
    drop(g);
    let g = open(&path);
    assert_eq!(mail_state(&g, handed), (false, false, false));
    assert_eq!(mail_state(&g, printed), (true, true, false));
}

#[test]
fn a_pending_relaunch_survives_a_daemon_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("loom.db");
    let g = open(&path);
    let lead = root(&g, "claude");
    let worker = agent_child(&g, lead, "worker");
    let to = NewAgent {
        agent: AgentId("codex".into()),
        choice: Default::default(),
    };
    g.plan_relaunch(lead, |_| {
        ((Some((Phase::AtTurnEnd, Plan::Switch(to.clone())))), ())
    })
    .unwrap();
    g.plan_relaunch(worker, |_| (Some((Phase::Running, Plan::Resume)), ()))
        .unwrap();
    g.limit_reached(worker).unwrap();
    drop(g);
    let g = open(&path);
    assert_eq!(
        g.relaunch_plan(lead).unwrap(),
        Some((Phase::AtTurnEnd, Plan::Switch(to)))
    );
    assert_eq!(
        g.relaunch_plan(worker).unwrap(),
        Some((Phase::AtTurnEnd, Plan::Resume)),
        "a relaunch that the old daemon ran waits for the next turn end"
    );
    assert!(g.limited(worker).unwrap());
    g.limit_cleared(worker).unwrap();
    assert!(!g.limited(worker).unwrap());
    g.remove(worker).unwrap();
    assert_eq!(g.relaunch_plan(worker).unwrap(), None);
}
