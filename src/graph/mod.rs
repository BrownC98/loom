mod agent_state;
mod closed;
mod deploys;
mod events;
mod gate;
mod hosts;
mod idle;
mod mailbox;
mod nodes;
mod relaunch_plans;
mod status;
mod token;
mod turns;
mod waits;
mod watchdog;

use std::ops::{Deref, DerefMut};
use std::sync::{Mutex, MutexGuard};

use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::futures::Notified;
use tokio::sync::{Notify, broadcast, mpsc};

use api::graph::{
    GraphEvent, NodeState, NodeView, Place, ProcessState, RelaunchOffer, Status, Toward,
};
use api::ids::{AgentId, NodeId, RepoId};

pub use agent_state::{AgentChange, AgentState};
pub use deploys::Deploy;
pub use gate::{BatchId, Ended, GateChoice, Merge, Outcome, Queued, Reclaim, Round, Unjudged};
pub use idle::IdleId;
pub use mailbox::{Kept, PARENT, Report, Sent};
pub use nodes::{Active, Launched, Listed, NewChild, Relations};
pub use relaunch_plans::{Attempt, DeafClaim, Hold, NewAgent, Next, Phase, Plan};
pub use token::{NodeLaunch, NodeToken};
pub use turns::{Nudge, TurnMark};
pub use waits::{HUMAN, WaitSource};

use crate::db::Database;
use api::graph::errors::Refusal;
use api::graph::events::{Entry, EventLog, EventRecord, Via, Who};
use api::graph::messages::{Kind, MessageKind};
use api::graph::waits::HumanWait;

pub(crate) const EVENT_QUEUE: usize = 256;

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Work {
    Running,
    Done {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kept: Option<String>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ProcessOutcome {
    Asleep,
    Exited { code: u32 },
    Lost,
    HostGone,
    SetupFailed { code: u32 },
}

fn node_state(work: Work, process: Option<ProcessOutcome>) -> NodeState {
    match (work, process) {
        (Work::Done { .. }, _) => NodeState::Done,
        (Work::Running, Some(ProcessOutcome::SetupFailed { code })) => {
            NodeState::SetupFailed { code }
        }
        (Work::Running, _) => NodeState::Running,
    }
}

fn process_state(process: Option<ProcessOutcome>, agent: Option<&AgentState>) -> ProcessState {
    match process {
        Some(ProcessOutcome::Asleep) => ProcessState::Asleep,
        Some(ProcessOutcome::Exited { code }) => ProcessState::Stopped { code: Some(code) },
        Some(
            ProcessOutcome::Lost | ProcessOutcome::HostGone | ProcessOutcome::SetupFailed { .. },
        ) => ProcessState::Stopped { code: None },
        None => agent.map_or(ProcessState::Working, AgentState::shown),
    }
}

pub struct Graph {
    db: Database,
    published: Mutex<u32>,
    events: broadcast::Sender<GraphEvent>,
    agent_tx: mpsc::UnboundedSender<AgentChange>,
    agent_rx: Mutex<Option<mpsc::UnboundedReceiver<AgentChange>>>,
    mail: Notify,
    linked: fn(&AgentId) -> bool,
}

pub(super) const REACHABLE: &str =
    "(process IS NULL OR process ->> 'kind' IN ('asleep', 'hostGone'))";

const VIEW_COLUMNS: &str = "SELECT n.id, n.name, n.agent, n.parent, n.state,
    (SELECT count(*) FROM messages m WHERE m.recipient = n.id AND m.delivered = 0 AND m.sent_at IS NULL),
    n.model, n.effort, n.repo, n.worktree,
    n.bookmark, n.wait,
    (SELECT json_group_array(json(d.wait)) FROM dones d WHERE d.node = n.id),
    (SELECT json_group_array(json(q.wait)) FROM questions q WHERE q.node = n.id),
    n.card_view, n.reply, n.process,
    EXISTS(SELECT 1 FROM relaunch_plans p WHERE p.node = n.id),
    n.session IS NOT NULL,
    (SELECT s.state FROM agent_states s WHERE s.node = n.id),
    n.limited,
    (SELECT s.wait FROM stalls s WHERE s.node = n.id AND s.dismissed = 0)";

struct Db<'a> {
    conn: MutexGuard<'a, Connection>,
    published: MutexGuard<'a, u32>,
    events: &'a broadcast::Sender<GraphEvent>,
    agent: &'a mpsc::UnboundedSender<AgentChange>,
}

impl Deref for Db<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.conn
    }
}

impl DerefMut for Db<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

impl Drop for Db<'_> {
    fn drop(&mut self) {
        let Ok(new) = events::after(&self.conn, *self.published) else {
            return;
        };
        let mut shown = false;
        for record in new {
            *self.published = record.id;
            if let Entry::ProcessChanged {
                node, from, to, by, ..
            } = &record.entry
            {
                shown |= from != to;
                if let Some(node) = node.id
                    && let Ok(to) = agent_state::state_of(&self.conn, node)
                {
                    let _ = self.agent.send(AgentChange {
                        node,
                        to,
                        by: by.clone(),
                    });
                }
            }
            let _ = self.events.send(GraphEvent::Recorded {
                record: Box::new(record),
            });
        }
        if shown {
            let _ = self.events.send(GraphEvent::Nodes);
        }
    }
}

impl Graph {
    pub fn open(db: Database, linked: fn(&AgentId) -> bool) -> rusqlite::Result<Self> {
        let published = events::last_id(&db.lock())?;
        relaunch_plans::settle_at_start(&db.lock())?;
        mailbox::requeue_handed(&db.lock())?;
        idle::forget_old_daemon(&db.lock())?;
        let (agent_tx, agent_rx) = mpsc::unbounded_channel();
        Ok(Self {
            db,
            published: Mutex::new(published),
            events: broadcast::channel(EVENT_QUEUE).0,
            agent_tx,
            agent_rx: Mutex::new(Some(agent_rx)),
            mail: Notify::new(),
            linked,
        })
    }

    pub fn settle(&self, home: &Place) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE nodes SET repo = ?1, worktree = ?2 WHERE repo IS NULL",
            params![home.repo.0, home.worktree],
        )?;
        Ok(())
    }

    pub fn place_of(&self, id: NodeId) -> rusqlite::Result<Place> {
        place_of(&self.lock(), id)
    }

    pub fn nodes_in(&self, worktree: &str) -> rusqlite::Result<Vec<NodeId>> {
        self.lock()
            .prepare("SELECT id FROM nodes WHERE worktree = ?1 ORDER BY id")?
            .query_map([worktree], |row| Ok(NodeId(row.get(0)?)))?
            .collect()
    }

    pub fn views(
        &self,
        tool: impl Fn(NodeId) -> Option<String>,
    ) -> rusqlite::Result<Vec<NodeView>> {
        status::views(&self.lock(), self.linked, &tool)
    }

    fn lock(&self) -> Db<'_> {
        Db {
            conn: self.db.lock(),
            published: self.published.lock().unwrap(),
            events: &self.events,
            agent: &self.agent_tx,
        }
    }

    pub fn agent_changes(&self) -> Option<mpsc::UnboundedReceiver<AgentChange>> {
        self.agent_rx.lock().unwrap().take()
    }

    pub fn event_log(&self) -> rusqlite::Result<EventLog> {
        events::log(&self.lock())
    }

    pub fn record_refusal(
        &self,
        from: NodeId,
        to: &str,
        how: Kind,
        what: Option<MessageKind>,
        reason: &Refusal,
        via: Option<&Via>,
    ) -> rusqlite::Result<()> {
        let db = self.lock();
        let entry = api::graph::events::Entry::Refused {
            how,
            what,
            from: events::who(&db, from)?,
            to: to.to_owned(),
            reason: reason.clone(),
            via: via.cloned(),
            gate: None,
        };
        events::write(&db, &entry, None)
    }

    pub fn record_about(
        &self,
        node: NodeId,
        entry: impl FnOnce(Who) -> Entry,
    ) -> rusqlite::Result<()> {
        let db = self.lock();
        let entry = entry(events::who(&db, node)?);
        events::write(&db, &entry, None)
    }

    pub fn process_log(&self, node: NodeId) -> rusqlite::Result<Vec<EventRecord>> {
        events::process_log(&self.lock(), node)
    }

    pub fn who(&self, node: NodeId) -> rusqlite::Result<Who> {
        events::who(&self.lock(), node)
    }

    pub fn record(&self, entry: &Entry) -> rusqlite::Result<()> {
        events::write(&self.lock(), entry, None)
    }

    pub fn publish_nodes(&self) {
        let _ = self.events.send(GraphEvent::Nodes);
    }

    pub fn mail_changed(&self) -> Notified<'_> {
        self.mail.notified()
    }

    pub fn wake_mail(&self) {
        self.mail.notify_waiters();
    }

    pub fn changes(&self) -> broadcast::Receiver<GraphEvent> {
        self.events.subscribe()
    }

    pub fn publish_view_changed(&self) {
        let _ = self.events.send(GraphEvent::ViewChanged);
    }

    fn publish_message(&self, child: NodeId, toward: Toward) {
        let _ = self.events.send(GraphEvent::Message { child, toward });
        self.mail.notify_waiters();
    }

    fn publish_hop(&self, from: NodeId, to: NodeId) {
        let _ = self.events.send(GraphEvent::Hop { from, to });
        self.mail.notify_waiters();
    }
}

fn view_from_row(row: &Row, linked: fn(&AgentId) -> bool) -> rusqlite::Result<NodeView> {
    let id = NodeId(row.get(0)?);
    let outcome = optional_json(row, 16)?;
    let state = node_state(from_json(row, 4)?, outcome);
    let process = process_state(outcome, optional_json::<AgentState>(row, 19)?.as_ref());
    let agent: Option<AgentId> = from_json(row, 2)?;
    let resumable = match (&state, outcome) {
        (NodeState::Done, _) => true,
        (
            _,
            Some(
                ProcessOutcome::Exited { .. }
                | ProcessOutcome::Lost
                | ProcessOutcome::SetupFailed { .. },
            ),
        ) => row.get(18)?,
        _ => true,
    };
    let offered = agent.as_ref().is_some_and(linked) && resumable;
    let relaunch = offered.then_some(match row.get(17)? {
        true => RelaunchOffer::Queued,
        false => RelaunchOffer::Ready,
    });
    let wait: Option<String> = row.get(11)?;
    let done: String = row.get(12)?;
    let asked: String = row.get(13)?;
    let card_view: Option<String> = row.get(14)?;
    let mut waits: Vec<HumanWait> = serde_json::from_str(&asked).unwrap_or_default();
    waits.sort_by_key(|w| w.id.0);
    waits.extend(wait.and_then(|w| {
        serde_json::from_str::<waits::Stored>(&w)
            .ok()
            .map(|s| s.wait)
    }));
    let stall: Option<String> = row.get(21)?;
    waits.extend(stall.and_then(|w| serde_json::from_str::<HumanWait>(&w).ok()));
    let mut done: Vec<HumanWait> = serde_json::from_str(&done).unwrap_or_default();
    done.sort_by_key(|w| w.id.0);
    waits.extend(done);
    let waits = waits.into_iter().map(waits::for_screen).collect();
    Ok(NodeView {
        id,
        name: row.get(1)?,
        agent,
        parent: row.get::<_, Option<u32>>(3)?.map(NodeId),
        state,
        process,
        unread: row.get(5)?,
        model: row.get(6)?,
        effort: row.get(7)?,
        place: Place {
            repo: RepoId(row.get(8)?),
            worktree: row.get(9)?,
        },
        bookmark: row.get(10)?,
        card_view: card_view.and_then(|v| serde_json::from_str(&v).ok()),
        reply: row.get(15)?,
        waits,
        relaunch,
        status: Status::Idle,
        since: 0,
        rework: 0,
    })
}

fn to_json(value: &impl Serialize) -> String {
    serde_json::to_string(value).expect("enum 값은 늘 직렬화된다")
}

fn from_json<T: DeserializeOwned>(row: &Row, idx: usize) -> rusqlite::Result<T> {
    let text: String = row.get(idx)?;
    serde_json::from_str(&text)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
}

fn optional_json<T: DeserializeOwned>(row: &Row, idx: usize) -> rusqlite::Result<Option<T>> {
    let text: Option<String> = row.get(idx)?;
    text.map(|text| {
        serde_json::from_str(&text)
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
    })
    .transpose()
}

fn state_of(db: &Connection, id: NodeId) -> rusqlite::Result<NodeState> {
    db.query_row(
        "SELECT state, process FROM nodes WHERE id = ?1",
        [id.0],
        |row| Ok(node_state(from_json(row, 0)?, optional_json(row, 1)?)),
    )
}

fn process_of(db: &Connection, id: NodeId) -> rusqlite::Result<Option<ProcessOutcome>> {
    db.query_row("SELECT process FROM nodes WHERE id = ?1", [id.0], |row| {
        optional_json(row, 0)
    })
    .optional()
    .map(Option::flatten)
}

fn parent_of(db: &Connection, id: NodeId) -> rusqlite::Result<Option<NodeId>> {
    db.query_row("SELECT parent FROM nodes WHERE id = ?1", [id.0], |row| {
        Ok(row.get::<_, Option<u32>>(0)?.map(NodeId))
    })
}

fn place_of(db: &Connection, id: NodeId) -> rusqlite::Result<Place> {
    db.query_row(
        "SELECT repo, worktree FROM nodes WHERE id = ?1",
        [id.0],
        |row| {
            Ok(Place {
                repo: RepoId(row.get(0)?),
                worktree: row.get(1)?,
            })
        },
    )
}

fn place_label(repo_path: &str, worktree: &str) -> String {
    format!("{}/{}", folder_name(repo_path), folder_name(worktree))
}

fn folder_name(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(path)
}

fn ambiguous(db: &Connection, name: &str, ids: &[NodeId]) -> rusqlite::Result<Refusal> {
    let mut found = db.prepare(
        "SELECT n.name, coalesce(r.path, ''), coalesce(n.worktree, '') FROM nodes n
         LEFT JOIN repos r ON r.id = n.repo WHERE n.id = ?1",
    )?;
    let candidates = ids
        .iter()
        .map(|id| {
            found.query_row([id.0], |row| {
                let place = place_label(&row.get::<_, String>(1)?, &row.get::<_, String>(2)?);
                Ok(format!("#{id} {} ({place})", row.get::<_, String>(0)?))
            })
        })
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Refusal::Ambiguous {
        name: name.to_owned(),
        candidates: candidates.join(", "),
    })
}

fn name_of(db: &Connection, id: NodeId) -> rusqlite::Result<String> {
    db.query_row("SELECT name FROM nodes WHERE id = ?1", [id.0], |row| {
        row.get(0)
    })
}

fn set_work(db: &Connection, id: NodeId, work: &Work) -> rusqlite::Result<()> {
    let from = state_of(db, id)?;
    db.execute(
        "UPDATE nodes SET state = ?2 WHERE id = ?1",
        params![id.0, to_json(work)],
    )?;
    note_state(db, id, from)
}

fn finish_work(db: &Connection, id: NodeId) -> rusqlite::Result<()> {
    let kept = db.query_row(
        "SELECT state ->> 'kept' FROM nodes WHERE id = ?1",
        [id.0],
        |row| row.get(0),
    )?;
    set_work(db, id, &Work::Done { kept })
}

fn running_live(db: &Connection, id: NodeId) -> rusqlite::Result<bool> {
    db.query_row(
        &format!("SELECT state ->> 'kind' = 'running' AND {REACHABLE} FROM nodes WHERE id = ?1"),
        [id.0],
        |row| row.get(0),
    )
}

fn note_state(db: &Connection, id: NodeId, from: NodeState) -> rusqlite::Result<()> {
    let to = state_of(db, id)?;
    if to == from {
        return Ok(());
    }
    let entry = api::graph::events::Entry::State {
        node: events::who(db, id)?,
        from,
        to,
    };
    events::write(db, &entry, None)
}
