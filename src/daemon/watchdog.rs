use std::collections::HashMap;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Notify;
use tokio::sync::broadcast::{self, error::RecvError, error::TryRecvError};

use super::App;
use super::relaunch::profile_of;
use super::sleep::{now, until};
use crate::graph::{AgentState, Quiet};
use api::graph::GraphEvent;
use api::ids::NodeId;

const IDLE_WAKE: Duration = Duration::from_secs(5 * 60);

const SETTLE: Duration = Duration::from_millis(300);

const IDLE_NOTE: &str = "You are idle with open work, and nothing will wake you. If a background command or Monitor of your own still runs and will notify you, end the turn. Else continue the task, or report with loom done or loom ask.";

#[derive(Default)]
pub struct Watchdog {
    listening: Mutex<HashMap<NodeId, Instant>>,
    jobs: Mutex<HashMap<NodeId, u32>>,
    changed: Notify,
}

impl Watchdog {
    pub fn saw(&self, node: NodeId, state: Option<&AgentState>) {
        let mut listening = self.listening.lock().unwrap();
        if state == Some(&AgentState::Listening) {
            listening.insert(node, now());
        } else {
            listening.remove(&node);
        }
        drop(listening);
        self.changed.notify_one();
    }

    pub fn quiet(&self, node: NodeId) -> Quiet {
        if self.jobs.lock().unwrap().contains_key(&node) {
            return Quiet::Job;
        }
        let listening = self.listening.lock().unwrap();
        match listening.get(&node) {
            Some(&since) if since + IDLE_WAKE <= now() => Quiet::Long,
            _ => Quiet::Short,
        }
    }

    pub fn jobs(&self, node: NodeId, delta: i32) {
        let mut jobs = self.jobs.lock().unwrap();
        let count = jobs.get(&node).map_or(0, |&n| i64::from(n)) + i64::from(delta);
        match u32::try_from(count) {
            Ok(n) if n > 0 => jobs.insert(node, n),
            _ => jobs.remove(&node),
        };
        drop(jobs);
        self.changed.notify_one();
    }

    pub fn forget_jobs(&self, node: NodeId) {
        if self.jobs.lock().unwrap().remove(&node).is_some() {
            self.changed.notify_one();
        }
    }

    fn due(&self, listening: &[NodeId], now: Instant) -> (Vec<NodeId>, Option<Instant>) {
        let mut since = self.listening.lock().unwrap();
        since.retain(|id, _| listening.contains(id));
        let mut due = Vec::new();
        let mut next: Option<Instant> = None;
        for &id in listening {
            let at = *since.entry(id).or_insert(now) + IDLE_WAKE;
            if at <= now {
                due.push(id);
            } else {
                next = Some(next.map_or(at, |next| next.min(at)));
            }
        }
        (due, next)
    }
}

async fn nodes_changed(events: &mut broadcast::Receiver<GraphEvent>) {
    loop {
        match events.recv().await {
            Ok(GraphEvent::Nodes) | Err(RecvError::Lagged(_)) => return,
            Ok(_) => {}
            Err(RecvError::Closed) => std::future::pending().await,
        }
    }
}

impl App {
    pub(super) fn watch_idle(self: &Arc<Self>) {
        let app = self.clone();
        let mut nodes = self.graph.changes();
        tokio::spawn(async move {
            loop {
                let mut mail = pin!(app.graph.mail_changed());
                mail.as_mut().enable();
                let mut changed = pin!(app.watchdog.changed.notified());
                changed.as_mut().enable();
                tokio::time::sleep(SETTLE).await;
                while !matches!(
                    nodes.try_recv(),
                    Err(TryRecvError::Empty | TryRecvError::Closed)
                ) {}
                let next = app.wake_idle().unwrap_or_else(|e| {
                    crate::log::logln!("loom: 일이 남은 채 쉬는 노드를 보지 못했다: {e}");
                    None
                });
                if let Err(e) = app.graph.sync_stalls(|node| app.watchdog.quiet(node)) {
                    crate::log::logln!("loom: 멈춘 노드의 알림을 맞추지 못했다: {e}");
                }
                tokio::select! {
                    () = mail => {}
                    () = changed => {}
                    () = nodes_changed(&mut nodes) => {}
                    () = until(next) => {}
                }
            }
        });
    }

    fn wake_idle(&self) -> rusqlite::Result<Option<Instant>> {
        let listening: Vec<NodeId> = self
            .graph
            .active()?
            .iter()
            .filter(|n| profile_of(n).is_some())
            .map(|n| n.id)
            .filter(|&id| self.agent_state(id) == Some(AgentState::Listening))
            .filter(|&id| !matches!(self.watchdog.quiet(id), Quiet::Job))
            .collect();
        let (due, next) = self.watchdog.due(&listening, now());
        for id in due {
            if self.graph.wake_idle(id, IDLE_NOTE)? {
                crate::log::logln!(
                    "loom: 일이 남은 채 쉬고 깨울 것이 없는 노드 {}를 깨운다",
                    id.0
                );
            }
        }
        Ok(next)
    }
}
