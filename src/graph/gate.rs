use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use super::events::{self, Entry, GateTrace, Via, Who};
use super::mailbox::{self, Stamp};
use super::{
    Graph, Kind, NodeId, REACHABLE, Refusal, from_json, name_of, parent_of, place_of, to_json,
};
use crate::profile::AgentId;
use crate::protocol::{
    self, COORDINATOR_ROLE, Finding, GateCall, GateFacts, GateState, GateVerb, Judgment, MergeEnd,
    MessageKind, Receiver, Sender, Target,
};
use crate::worktree::RepoId;

pub struct Round<'a> {
    pub branch: &'a str,
    pub target: &'a str,
    pub worktree: &'a str,
    pub head: &'a str,
}

#[derive(Debug, PartialEq)]
pub enum GateChoice {
    Coordinator { gate: NodeId, worktree: String },
    Owner,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Queued {
    pub branch: String,
    pub owner: NodeId,
    pub head: String,
    pub since: i64,
    pub waited: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BatchId(pub i64);

#[derive(Debug, PartialEq)]
pub struct Stale {
    pub id: BatchId,
    pub repo: RepoId,
    pub target: String,
    pub base: Option<String>,
    pub tip: Option<String>,
    pub members: Vec<Queued>,
}

pub struct Ended<'a> {
    pub end: MergeEnd,
    pub merged: Option<&'a str>,
    pub cause: Option<String>,
    pub log: Option<String>,
    pub lines: Vec<String>,
    pub findings: Vec<Finding>,
}

#[derive(Serialize, Deserialize)]
struct Verdict {
    judgment: Judgment,
    findings: Option<String>,
    reviewer: Who,
    model: Option<String>,
}

struct Row {
    owner: NodeId,
    gate: NodeId,
    target: String,
    worktree: String,
    state: GateState,
    head: String,
    mark: u32,
}

impl Row {
    fn target_kind(&self, db: &Connection) -> rusqlite::Result<Target> {
        let coordinator: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM nodes WHERE id = ?1 AND id != ?2 AND role = ?3)",
            params![self.gate.0, self.owner.0, COORDINATOR_ROLE],
            |row| row.get(0),
        )?;
        Ok(match coordinator {
            true => Target::Coordinator,
            false => Target::Base,
        })
    }
}

#[derive(Default)]
struct Detail {
    findings_file: Option<String>,
    sha: Option<String>,
    cause: Option<String>,
    log: Option<String>,
    lines: Vec<String>,
    findings: Vec<Finding>,
}

#[derive(Clone, Copy)]
enum Caller {
    Node(NodeId),
    Loom,
}

impl Caller {
    fn node(self) -> Option<NodeId> {
        match self {
            Caller::Node(id) => Some(id),
            Caller::Loom => None,
        }
    }
}

impl Graph {
    pub fn gate_for(&self, owner: NodeId) -> Result<GateChoice, Refusal> {
        let db = self.lock();
        match coordinator_parent(&db, owner)? {
            Some(parent) => Ok(GateChoice::Coordinator {
                gate: parent,
                worktree: place_of(&db, parent)?.worktree,
            }),
            None => Ok(GateChoice::Owner),
        }
    }

    pub fn ready(
        &self,
        owner: NodeId,
        round: &Round,
        gate: NodeId,
        via: Option<&Via>,
    ) -> Result<String, Refusal> {
        let detail = Detail::default();
        let repo = place_of(&self.lock(), owner)?.repo;
        let row = self.apply(
            Caller::Node(owner),
            repo,
            round.branch,
            GateCall::Ready,
            detail,
            via,
            |tx, row| {
                tx.execute(
                    "INSERT INTO gates (repo, branch, owner, gate, target, worktree, state, head, opened, updated, mark)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, unixepoch(), unixepoch(), (SELECT max(id) FROM nodes))
                     ON CONFLICT (repo, branch) DO UPDATE SET owner = ?3, gate = ?4, target = ?5, worktree = ?6,
                        state = ?7, head = ?8, verdict = NULL, queued = NULL, seq = NULL, batch = NULL, opened = unixepoch(),
                        updated = unixepoch(), mark = (SELECT max(id) FROM nodes)",
                    params![
                        repo.0,
                        round.branch,
                        owner.0,
                        gate.0,
                        round.target,
                        round.worktree,
                        to_json(&row.state),
                        round.head
                    ],
                )?;
                Ok(())
            },
        )?;
        Ok(name_of(&self.lock(), row.gate)?)
    }

    pub fn announce_ready(
        &self,
        owner: NodeId,
        round: &Round,
        gate: NodeId,
        via: Option<&Via>,
    ) -> Result<String, Refusal> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let what = MessageKind::Ready {
            branch: round.branch.to_owned(),
            head: round.head.to_owned(),
            worktree: round.worktree.to_owned(),
            target: round.target.to_owned(),
        };
        let stamp = Stamp {
            via: via.cloned(),
            gate: Some(trace(
                &tx,
                Caller::Node(owner),
                GateVerb::Ready,
                round.branch,
                None,
                None,
                Vec::new(),
            )?),
        };
        let body = format!(
            "Next: review the branch, merge it into {} by hand, then report with `loom done`. This repository has no gate scripts, so loom verdict and the merge queue do not work here.",
            round.target
        );
        mailbox::enqueue(&tx, owner, Some(gate), Kind::Queue, &what, &body, stamp)?;
        tx.commit()?;
        self.publish_sent(&db, owner, gate)?;
        Ok(name_of(&db, gate)?)
    }

    pub fn under_coordinator(&self, node: NodeId) -> rusqlite::Result<bool> {
        Ok(coordinator_parent(&self.lock(), node)?.is_some())
    }

    pub fn ready_sent(&self, owner: NodeId, branch: &str, head: &str) -> rusqlite::Result<bool> {
        self.lock().query_row(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE sender = ?1 AND what ->> 'kind' = 'ready'
             AND what ->> 'branch' = ?2 AND what ->> 'head' = ?3)",
            params![owner.0, branch, head],
            |row| row.get(0),
        )
    }

    pub fn verdict(
        &self,
        caller: NodeId,
        branch: &str,
        judgment: Judgment,
        findings: Option<&str>,
        via: Option<&Via>,
    ) -> Result<GateState, Refusal> {
        if judgment == Judgment::Block && findings.is_none() {
            return Err(Refusal::NoFindings);
        }
        let detail = Detail {
            findings_file: findings.map(str::to_owned),
            ..Detail::default()
        };
        let repo = place_of(&self.lock(), caller)?.repo;
        let row = self.apply(
            Caller::Node(caller),
            repo,
            branch,
            GateCall::Verdict(judgment),
            detail,
            via,
            |tx, row| {
                let verdict = Verdict {
                    judgment,
                    findings: findings.map(str::to_owned),
                    reviewer: events::who(tx, caller)?,
                    model: model_of(tx, caller)?,
                };
                update(tx, repo, branch, row.state, Some(&to_json(&verdict)), None)?;
                tx.execute(
                    "UPDATE gates SET queued = CASE WHEN ?3 = '\"passed\"' THEN coalesce(queued, unixepoch()) END,
                        seq = CASE WHEN ?3 = '\"passed\"' THEN coalesce(seq, (SELECT coalesce(max(seq), 0) + 1 FROM gates)) END
                     WHERE repo = ?1 AND branch = ?2",
                    params![repo.0, branch, to_json(&row.state)],
                )?;
                Ok(())
            },
        )?;
        Ok(row.state)
    }

    pub fn gate_merges(&self, repo: RepoId, target: &str) -> rusqlite::Result<Vec<String>> {
        self.lock()
            .prepare(
                "SELECT merged FROM gates WHERE repo = ?1 AND target = ?2 AND merged IS NOT NULL",
            )?
            .query_map(params![repo.0, target], |r| r.get(0))?
            .collect()
    }

    pub fn audit(&self, owner: NodeId, since: i64) -> Result<Vec<Finding>, Refusal> {
        let db = self.lock();
        let mut findings = Vec::new();
        if let Some(oldest) = events::oldest_at(&db)?
            && oldest > since
        {
            findings.push(Finding::RangeIncomplete { since, oldest });
        }
        let tree = tree_of(&db, owner)?;
        for record in events::sent_since(&db, since)? {
            let Entry::Message {
                from,
                gate: None,
                what,
                ..
            } = &record.entry
            else {
                continue;
            };
            if what.as_ref().is_some_and(protocol::MessageKind::by_loom) {
                continue;
            }
            let in_tree = from.id.is_some_and(|id| tree.contains(&id));
            if let (true, Some(body)) = (in_tree, &record.body)
                && let Some(word) = protocol::gate_word(body)
            {
                findings.push(Finding::RawGateWord {
                    event: record.id,
                    from: from.name.clone(),
                    word: word.to_owned(),
                });
            }
        }
        Ok(findings)
    }

    pub fn fail(
        &self,
        caller: NodeId,
        branch: &str,
        cause: &str,
        via: Option<&Via>,
    ) -> Result<GateState, Refusal> {
        let detail = Detail {
            cause: Some(cause.to_owned()),
            ..Detail::default()
        };
        let repo = place_of(&self.lock(), caller)?.repo;
        let row = self.apply(
            Caller::Node(caller),
            repo,
            branch,
            GateCall::Fail,
            detail,
            via,
            |tx, row| Ok(update(tx, repo, branch, row.state, None, None)?),
        )?;
        Ok(row.state)
    }

    pub fn record_gate_refusal(
        &self,
        caller: NodeId,
        verb: GateVerb,
        branch: &str,
        reason: &Refusal,
        via: Option<&Via>,
    ) -> rusqlite::Result<()> {
        let db = self.lock();
        let repo = place_of(&db, caller)?.repo;
        let from = load(&db, repo, branch)?.map(|row| row.state);
        let entry = Entry::Refused {
            what: None,
            how: Kind::Queue,
            from: events::who(&db, caller)?,
            to: branch.to_owned(),
            reason: reason.clone(),
            via: via.cloned(),
            gate: Some(trace(
                &db,
                Caller::Node(caller),
                verb,
                branch,
                from,
                None,
                Vec::new(),
            )?),
        };
        events::write(&db, &entry, None)
    }

    pub fn queued_targets(&self) -> rusqlite::Result<Vec<(RepoId, String)>> {
        self.lock()
            .prepare(&format!(
                "SELECT repo, target FROM gates WHERE {} GROUP BY repo, target ORDER BY min(seq)",
                settled()
            ))?
            .query_map([], |r| Ok((RepoId(r.get(0)?), r.get(1)?)))?
            .collect()
    }

    pub fn queue(&self, repo: RepoId, target: &str) -> rusqlite::Result<Vec<Queued>> {
        self.lock()
            .prepare(&format!(
                "{QUEUED} WHERE repo = ?1 AND target = ?2 AND {} ORDER BY seq",
                settled()
            ))?
            .query_map(params![repo.0, target], read_queued)?
            .collect()
    }

    pub fn batch_open(
        &self,
        repo: RepoId,
        target: &str,
        branches: &[&str],
    ) -> Result<BatchId, Refusal> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        tx.execute(
            "INSERT INTO batches (repo, target, started) VALUES (?1, ?2, unixepoch())",
            params![repo.0, target],
        )?;
        let id = BatchId(tx.last_insert_rowid());
        for branch in branches {
            let row = load(&tx, repo, branch)?.ok_or_else(|| Refusal::NoRound((*branch).into()))?;
            let to = protocol::step(row.state, GateCall::Start)?;
            tx.execute(
                "UPDATE gates SET state = ?3, batch = ?4, updated = unixepoch() WHERE repo = ?1 AND branch = ?2",
                params![repo.0, branch, to_json(&to), id.0],
            )?;
        }
        tx.commit()?;
        Ok(id)
    }

    pub fn batch_checked(&self, id: BatchId, base: &str, tip: &str) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE batches SET base = ?2, tip = ?3 WHERE id = ?1",
            params![id.0, base, tip],
        )?;
        Ok(())
    }

    pub fn batch_close(&self, id: BatchId) -> rusqlite::Result<()> {
        self.lock().execute(
            "DELETE FROM batches WHERE id = ?1
             AND NOT EXISTS (SELECT 1 FROM gates WHERE batch = ?1 AND state = '\"merging\"')",
            [id.0],
        )?;
        Ok(())
    }

    pub fn stale_batches(&self) -> rusqlite::Result<Vec<Stale>> {
        let db = self.lock();
        let mut stale: Vec<Stale> = db
            .prepare("SELECT id, repo, target, base, tip FROM batches ORDER BY id")?
            .query_map([], |r| {
                Ok(Stale {
                    id: BatchId(r.get(0)?),
                    repo: RepoId(r.get(1)?),
                    target: r.get(2)?,
                    base: r.get(3)?,
                    tip: r.get(4)?,
                    members: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        for batch in &mut stale {
            batch.members = db
                .prepare(&format!(
                    "{QUEUED} WHERE batch = ?1 AND state = '\"merging\"' ORDER BY seq"
                ))?
                .query_map([batch.id.0], read_queued)?
                .collect::<rusqlite::Result<_>>()?;
        }
        Ok(stale)
    }

    pub fn orphans(&self) -> rusqlite::Result<Vec<(RepoId, String)>> {
        self.lock()
            .prepare("SELECT repo, branch FROM gates WHERE state = '\"merging\"' AND batch IS NULL ORDER BY rowid")?
            .query_map([], |r| Ok((RepoId(r.get(0)?), r.get(1)?)))?
            .collect()
    }

    pub fn requeue(&self, repo: RepoId, branch: &str) -> Result<GateState, Refusal> {
        let row = self.apply(
            Caller::Loom,
            repo,
            branch,
            GateCall::Requeue,
            Detail::default(),
            None,
            |tx, row| {
                update(tx, repo, branch, row.state, None, None)?;
                tx.execute(
                    "UPDATE gates SET batch = NULL WHERE repo = ?1 AND branch = ?2",
                    params![repo.0, branch],
                )?;
                Ok(())
            },
        )?;
        Ok(row.state)
    }

    pub fn merge_end(
        &self,
        repo: RepoId,
        branch: &str,
        ended: Ended,
    ) -> Result<GateState, Refusal> {
        let Ended {
            end,
            merged,
            cause,
            log,
            mut lines,
            findings,
        } = ended;
        lines.extend(findings.iter().map(|f| format!("audit: {f}")));
        let detail = Detail {
            sha: merged.map(str::to_owned),
            cause,
            log,
            lines,
            findings,
            ..Detail::default()
        };
        let row = self.apply(
            Caller::Loom,
            repo,
            branch,
            GateCall::End(end),
            detail,
            None,
            |tx, row| {
                update(tx, repo, branch, row.state, None, merged)?;
                tx.execute(
                    "UPDATE gates SET batch = NULL, queued = NULL, seq = NULL WHERE repo = ?1 AND branch = ?2",
                    params![repo.0, branch],
                )?;
                Ok(())
            },
        )?;
        Ok(row.state)
    }

    #[allow(clippy::too_many_arguments)]
    fn apply(
        &self,
        caller: Caller,
        repo: RepoId,
        branch: &str,
        call: GateCall,
        detail: Detail,
        via: Option<&Via>,
        write: impl FnOnce(&Transaction, &Row) -> Result<(), Refusal>,
    ) -> Result<Row, Refusal> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let old = load(&tx, repo, branch)?;
        let from = old.as_ref().map_or(GateState::Working, |row| row.state);
        allowed_caller(&tx, caller, old.as_ref(), call.verb(), branch)?;
        let to = protocol::step(from, call)?;
        let row = match old {
            Some(row) => Row { state: to, ..row },
            None => Row {
                owner: caller.node().expect("only loom ready opens a row"),
                gate: caller.node().expect("only loom ready opens a row"),
                target: String::new(),
                worktree: String::new(),
                state: to,
                head: String::new(),
                mark: 0,
            },
        };
        write(&tx, &row)?;
        let row = load(&tx, repo, branch)?.expect("방금 쓴 관문 줄");
        let trace = trace(
            &tx,
            caller,
            call.verb(),
            branch,
            Some(from),
            Some(to),
            detail.findings,
        )?;
        let mut sent = Vec::new();
        if let Some(what) = protocol::gate_kind(
            call,
            GateFacts {
                branch: branch.to_owned(),
                head: row.head.clone(),
                worktree: row.worktree.clone(),
                target: row.target.clone(),
                findings: detail.findings_file,
                reviewer: trace.agent.as_ref().map(|agent| match &trace.model {
                    Some(model) => format!("{} {model}", agent.0),
                    None => agent.0.clone(),
                }),
                sha: detail.sha,
                cause: detail.cause,
                log: detail.log,
            },
        ) {
            for receiver in protocol::receivers(call, row.target_kind(&tx)?) {
                let to_node = match receiver {
                    Receiver::Gate => row.gate,
                    Receiver::Owner => row.owner,
                    Receiver::Root if exists(&tx, row.owner)? => root_of(&tx, row.owner)?,
                    Receiver::Root => continue,
                };
                if Some(to_node) == caller.node()
                    || sent.contains(&to_node)
                    || !exists(&tx, to_node)?
                {
                    continue;
                }
                let mut body = detail.lines.clone();
                body.push(protocol::next_step(call, *receiver, branch));
                post(
                    &tx,
                    caller,
                    to_node,
                    Kind::Queue,
                    &what,
                    &body.join("\n"),
                    via,
                    &trace,
                )?;
                sent.push(to_node);
            }
            if let (true, Caller::Node(me)) = (sent.is_empty(), caller) {
                let mut body = detail.lines;
                body.push(protocol::KEPT_FOR_CALLER.to_owned());
                post(
                    &tx,
                    caller,
                    me,
                    Kind::Queue,
                    &what,
                    &body.join("\n"),
                    via,
                    &trace,
                )?;
                sent.push(me);
            }
        }
        if withdrawn(from, to) {
            sent.extend(withdraw(&tx, caller, &row, branch, &trace)?);
        }
        tx.commit()?;
        for to_node in sent {
            match caller {
                Caller::Node(me) => self.publish_sent(&db, me, to_node)?,
                Caller::Loom => self.publish_node(&db, to_node)?,
            }
        }
        if let Caller::Loom = caller {
            self.mail.notify_waiters();
        }
        Ok(row)
    }
}

fn allowed_caller(
    db: &Connection,
    caller: Caller,
    row: Option<&Row>,
    verb: GateVerb,
    branch: &str,
) -> Result<(), Refusal> {
    let not_gate = |row: &Row| -> Result<(), Refusal> {
        Err(Refusal::NotGate {
            branch: branch.to_owned(),
            gate: name_of(db, row.gate)?,
        })
    };
    match (caller, row, protocol::sender(verb)) {
        (Caller::Loom, _, Sender::Loom) | (Caller::Node(_), _, Sender::Owner) => Ok(()),
        (_, None, _) => Err(Refusal::NoRound(branch.to_owned())),
        (Caller::Loom, Some(row), _) => not_gate(row),
        (Caller::Node(_), Some(row), Sender::Loom) => not_gate(row),
        (Caller::Node(me), Some(row), Sender::Reviewer) if row.owner == me => {
            Err(Refusal::OwnVerdict(branch.to_owned()))
        }
        (Caller::Node(me), Some(row), Sender::Reviewer) => match parent_of(db, me)? {
            Some(parent) if parent == row.gate => Ok(()),
            _ => not_gate(row),
        },
        (Caller::Node(me), Some(row), Sender::Gate) if row.gate == me => Ok(()),
        (Caller::Node(_), Some(row), Sender::Gate) => not_gate(row),
    }
}

fn withdrawn(from: GateState, to: GateState) -> bool {
    matches!(
        (from, to),
        (
            GateState::Ready | GateState::Passed | GateState::Merging,
            GateState::Working | GateState::Failed
        )
    )
}

fn withdraw(
    tx: &Transaction,
    caller: Caller,
    row: &Row,
    branch: &str,
    trace: &GateTrace,
) -> rusqlite::Result<Vec<NodeId>> {
    let reviewers: Vec<(NodeId, Option<AgentId>)> = tx
        .prepare(&format!(
            "SELECT n.id, n.agent FROM nodes n WHERE {} AND n.id != ?3 ORDER BY n.id",
            in_round("?1", "?2", "?4", "?5")
        ))?
        .query_map(
            params![
                row.gate.0,
                row.owner.0,
                caller.node().map_or(0, |n| n.0),
                row.mark,
                branch
            ],
            |r| Ok((NodeId(r.get(0)?), from_json(r, 1)?)),
        )?
        .collect::<rusqlite::Result<_>>()?;
    let what = MessageKind::Withdrawn {
        branch: branch.to_owned(),
        head: row.head.clone(),
    };
    let body = protocol::withdrawn_step(branch, row.state);
    for (node, agent) in &reviewers {
        let kind = match agent.as_ref().and_then(crate::profile::profile_of) {
            Some(_) => Kind::Steer,
            None => Kind::Queue,
        };
        mailbox::enqueue_from_loom(tx, *node, kind, &what, &body, Some(trace.clone()))?;
    }
    Ok(reviewers.into_iter().map(|(node, _)| node).collect())
}

fn reviewer(gate: &str, owner: &str) -> String {
    format!(
        "n.parent = {gate} AND n.id != {owner} AND {REACHABLE} AND n.task IS (SELECT task FROM nodes WHERE id = {gate})"
    )
}

fn in_round(gate: &str, owner: &str, mark: &str, branch: &str) -> String {
    format!(
        "({} AND (n.id > {mark} OR {}))",
        reviewer(gate, owner),
        judged(branch)
    )
}

fn done(owner: &str, mark: &str, branch: &str) -> String {
    format!(
        "({} OR (n.id > {mark} AND {}))",
        voted(branch, owner),
        judged_any()
    )
}

fn judged(branch: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM events e WHERE e.entry ->> '$.kind' = 'message'
            AND e.entry ->> '$.gate.verb' = 'verdict' AND e.entry ->> '$.gate.branch' = {branch}
            AND e.entry ->> '$.from.id' = n.id)"
    )
}

fn judged_any() -> String {
    "EXISTS (SELECT 1 FROM events e WHERE e.entry ->> '$.kind' = 'message'
        AND e.entry ->> '$.gate.verb' = 'verdict' AND e.entry ->> '$.from.id' = n.id)"
        .to_owned()
}

fn voted(branch: &str, owner: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM events e WHERE e.entry ->> '$.kind' = 'message'
            AND e.entry ->> '$.gate.verb' = 'verdict' AND e.entry ->> '$.gate.branch' = {branch}
            AND e.entry ->> '$.from.id' = n.id
            AND e.id > (SELECT max(r.id) FROM events r WHERE r.entry ->> '$.kind' = 'message'
                AND r.entry ->> '$.gate.verb' = 'ready' AND r.entry ->> '$.gate.branch' = {branch}
                AND r.entry ->> '$.from.id' = {owner}))"
    )
}

#[allow(clippy::too_many_arguments)]
fn post(
    tx: &Transaction,
    caller: Caller,
    to: NodeId,
    kind: Kind,
    what: &MessageKind,
    body: &str,
    via: Option<&Via>,
    trace: &GateTrace,
) -> rusqlite::Result<()> {
    match caller {
        Caller::Node(me) => {
            let stamp = Stamp {
                via: via.cloned(),
                gate: Some(trace.clone()),
            };
            mailbox::enqueue(tx, me, Some(to), kind, what, body, stamp)?;
        }
        Caller::Loom => {
            mailbox::enqueue_from_loom(tx, to, kind, what, body, Some(trace.clone()))?;
        }
    }
    Ok(())
}

fn coordinator_parent(db: &Connection, node: NodeId) -> rusqlite::Result<Option<NodeId>> {
    Ok(match parent_of(db, node)? {
        Some(parent) if role_of(db, parent)?.as_deref() == Some(COORDINATOR_ROLE) => Some(parent),
        _ => None,
    })
}

fn role_of(db: &Connection, node: NodeId) -> rusqlite::Result<Option<String>> {
    db.query_row("SELECT role FROM nodes WHERE id = ?1", [node.0], |row| {
        row.get(0)
    })
}

fn model_of(db: &Connection, node: NodeId) -> rusqlite::Result<Option<String>> {
    db.query_row("SELECT model FROM nodes WHERE id = ?1", [node.0], |row| {
        row.get(0)
    })
}

fn exists(db: &Connection, node: NodeId) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM nodes WHERE id = ?1)",
        [node.0],
        |row| row.get(0),
    )
}

fn settled() -> String {
    format!(
        "state = '\"passed\"' AND NOT EXISTS (SELECT 1 FROM nodes n WHERE {} AND NOT {})",
        in_round("gates.gate", "gates.owner", "gates.mark", "gates.branch"),
        done("gates.owner", "gates.mark", "gates.branch")
    )
}

const QUEUED: &str = "SELECT branch, owner, head, opened, max(unixepoch() - coalesce(queued, unixepoch()), 0) FROM gates";

fn read_queued(r: &rusqlite::Row) -> rusqlite::Result<Queued> {
    Ok(Queued {
        branch: r.get(0)?,
        owner: NodeId(r.get(1)?),
        head: r.get(2)?,
        since: r.get(3)?,
        waited: r.get::<_, i64>(4)?.unsigned_abs(),
    })
}

fn load(db: &Connection, repo: RepoId, branch: &str) -> rusqlite::Result<Option<Row>> {
    db.query_row(
        "SELECT owner, gate, target, worktree, state, head, mark FROM gates WHERE repo = ?1 AND branch = ?2",
        params![repo.0, branch],
        |row| {
            Ok(Row {
                owner: NodeId(row.get(0)?),
                gate: NodeId(row.get(1)?),
                target: row.get(2)?,
                worktree: row.get(3)?,
                state: from_json(row, 4)?,
                head: row.get(5)?,
                mark: row.get(6)?,
            })
        },
    )
    .optional()
}

fn update(
    db: &Connection,
    repo: RepoId,
    branch: &str,
    state: GateState,
    verdict: Option<&str>,
    merged: Option<&str>,
) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE gates SET state = ?3, verdict = coalesce(?4, verdict), merged = coalesce(?5, merged), updated = unixepoch()
         WHERE repo = ?1 AND branch = ?2",
        params![repo.0, branch, to_json(&state), verdict, merged],
    )?;
    Ok(())
}

fn trace(
    db: &Connection,
    caller: Caller,
    verb: GateVerb,
    branch: &str,
    from: Option<GateState>,
    to: Option<GateState>,
    findings: Vec<Finding>,
) -> rusqlite::Result<GateTrace> {
    let (role, agent, model) = match caller {
        Caller::Node(node) => db.query_row(
            "SELECT role, agent, model FROM nodes WHERE id = ?1",
            [node.0],
            |row| Ok((row.get(0)?, from_json(row, 1)?, row.get(2)?)),
        )?,
        Caller::Loom => (None, None, None),
    };
    Ok(GateTrace {
        verb,
        branch: branch.to_owned(),
        from,
        to,
        role,
        agent,
        model,
        findings,
    })
}

fn root_of(db: &Connection, node: NodeId) -> rusqlite::Result<NodeId> {
    let mut at = node;
    while let Some(parent) = parent_of(db, at)? {
        at = parent;
    }
    Ok(at)
}

fn tree_of(db: &Connection, top: NodeId) -> rusqlite::Result<Vec<NodeId>> {
    db.prepare(
        "WITH RECURSIVE tree(id) AS (SELECT ?1 UNION SELECT n.id FROM nodes n JOIN tree t ON n.parent = t.id)
         SELECT t.id FROM tree t JOIN nodes n ON n.id = t.id WHERE n.repo = (SELECT repo FROM nodes WHERE id = ?1)",
    )?
    .query_map([top.0], |row| Ok(NodeId(row.get(0)?)))?
    .collect()
}
