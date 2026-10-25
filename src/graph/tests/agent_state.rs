use super::*;

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
    g.lost(worker).unwrap();
    g.sleep_lost(worker).unwrap();
    assert_eq!(
        changes(&g, worker),
        [
            (AgentState::Listening, AgentEvent::WaitOpened),
            (AgentState::Asleep, AgentEvent::Slept),
            (AgentState::Starting, AgentEvent::Relaunched),
            (AgentState::Relaunching, AgentEvent::RelaunchStarted),
            (AgentState::Ended, AgentEvent::Exited),
        ]
    );
    g.exited(lead, 0).unwrap();
    assert_eq!(g.agent_state(lead).unwrap(), Some(AgentState::Ended));
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
    g.compare_busy(ended, false).unwrap();
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

#[test]
fn a_disagreement_is_recorded_once_per_state() {
    let dir = tempfile::tempdir().unwrap();
    let g = open(&dir.path().join("loom.db"));
    let lead = root(&g, "claude");
    g.agent_event(lead, AgentEvent::WaitOpened).unwrap();
    g.compare_busy(lead, false).unwrap();
    g.compare_listening(lead, true).unwrap();
    assert_eq!(agent_entries(&g, lead).len(), 1, "both agree");
    g.agent_event(lead, AgentEvent::WaitClosed { by: IdleEnd::Mail })
        .unwrap();
    g.compare_busy(lead, false).unwrap();
    g.compare_busy(lead, false).unwrap();
    g.compare_listening(lead, true).unwrap();
    g.agent_event(lead, AgentEvent::TurnEnded).unwrap();
    g.compare_listening(lead, true).unwrap();
    let disagreed: Vec<(ShadowCheck, bool, AgentState)> = agent_entries(&g, lead)
        .into_iter()
        .filter_map(|e| match e {
            Entry::AgentStateDisagreed {
                check, old, state, ..
            } => Some((check, old, state)),
            _ => None,
        })
        .collect();
    assert_eq!(
        disagreed,
        [
            (ShadowCheck::Busy, false, AgentState::Turn),
            (ShadowCheck::Listening, true, AgentState::Deaf),
        ]
    );
}
