use std::collections::HashMap;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

use super::App;
use super::relaunch::profile_of;
use super::sleep::{now, until};
use crate::graph::AgentState;
use api::ids::NodeId;

const IDLE_WAKE: Duration = Duration::from_secs(30 * 60);

const IDLE_NOTE: &str = "You are idle with open work, and nothing will wake you. If a background command or Monitor of your own still runs and will notify you, end the turn. Else continue the task, or report with loom done or loom ask.";

#[derive(Default)]
pub struct Watchdog {
    listening: Mutex<HashMap<NodeId, Instant>>,
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

impl App {
    pub(super) fn watch_idle(self: &Arc<Self>) {
        let app = self.clone();
        tokio::spawn(async move {
            loop {
                let mut mail = pin!(app.graph.mail_changed());
                mail.as_mut().enable();
                let mut changed = pin!(app.watchdog.changed.notified());
                changed.as_mut().enable();
                let next = app.wake_idle().unwrap_or_else(|e| {
                    crate::log::logln!("loom: 일이 남은 채 쉬는 노드를 보지 못했다: {e}");
                    None
                });
                tokio::select! {
                    () = mail => {}
                    () = changed => {}
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
