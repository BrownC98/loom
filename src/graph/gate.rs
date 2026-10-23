use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use super::events::{self, Entry, GateTrace, Via, Who};
use super::mailbox::{self, Stamp};
use super::{
    Graph, Kind, NodeId, REACHABLE, Refusal, from_json, name_of, parent_of, place_of, to_json,
};
use crate::protocol::{
    self, CI_ROLE, COORDINATOR_ROLE, Finding, GateCall, GateFacts, GateState, GateVerb, Judgment,
    MergeEnd, MessageKind, Receiver, Sender,
};

pub struct Round<'a> {
    pub branch: &'a str,
    pub target: &'a str,
    pub worktree: &'a str,
    pub head: &'a str,
}

#[derive(Debug, PartialEq)]
pub enum GateChoice {
    Coordinator { gate: NodeId, worktree: String },
    Ci { gate: NodeId },
}

#[derive(Debug, PartialEq)]
pub struct MergeTask {
    pub owner: NodeId,
    pub worktree: String,
    pub target: String,
    pub head: String,
    pub since: i64,
    pub run: i64,
    pub from: GateState,
    pub merged: Option<String>,
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
    since: i64,
    run: i64,
    merged: Option<String>,
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

impl Graph {
    pub fn gate_for(&self, owner: NodeId) -> Result<GateChoice, Refusal> {
        let db = self.lock();
        if let Some(parent) = coordinator_parent(&db, owner)? {
            return Ok(GateChoice::Coordinator {
                gate: parent,
                worktree: place_of(&db, parent)?.worktree,
            });
        }
        let repo = place_of(&db, owner)?.repo;
        let found = db
            .prepare(
                &format!("SELECT id FROM nodes WHERE role = ?1 AND repo = ?2 AND {REACHABLE} AND id != ?3 ORDER BY id"),
            )?
            .query_map(params![CI_ROLE, repo.0, owner.0], |row| {
                Ok(NodeId(row.get(0)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        match found.as_slice() {
            [gate] => Ok(GateChoice::Ci { gate: *gate }),
            [] => Err(Refusal::NoGate),
            many => Err(Refusal::ManyGates(
                many.iter()
                    .map(|id| format!("#{id}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            )),
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
        let row = self.apply(owner, round.branch, GateCall::Ready, detail, via, |tx, row| {
            let repo = place_of(tx, owner)?.repo;
            tx.execute(
                "INSERT INTO gates (repo, branch, owner, gate, target, worktree, state, head, opened, updated)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, unixepoch(), unixepoch())
                 ON CONFLICT (repo, branch) DO UPDATE SET owner = ?3, gate = ?4, target = ?5, worktree = ?6,
                    state = ?7, head = ?8, verdict = NULL, opened = unixepoch(), updated = unixepoch()",
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
        })?;
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
                owner,
                GateVerb::Ready,
                round.branch,
                None,
                None,
                Vec::new(),
            )?),
        };
        let body = format!(
            "Next: review the branch, merge it into {} by hand, then report with `loom done`. This repository has no gate scripts, so loom verdict and loom merge do not work here.",
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
        let row = self.apply(
            caller,
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
                Ok(update(
                    tx,
                    caller,
                    branch,
                    row.state,
                    Some(&to_json(&verdict)),
                    None,
                )?)
            },
        )?;
        Ok(row.state)
    }

    pub fn merge_start(
        &self,
        caller: NodeId,
        branch: &str,
        via: Option<&Via>,
    ) -> Result<MergeTask, Refusal> {
        let detail = Detail::default();
        let mut from = GateState::Working;
        let row = self.apply(
            caller,
            branch,
            GateCall::MergeStart,
            detail,
            via,
            |tx, row| {
                from = load(tx, caller, branch)?.map_or(from, |old| old.state);
                update(tx, caller, branch, row.state, None, None)?;
                tx.execute(
                    "UPDATE gates SET run = run + 1 WHERE repo = ?1 AND branch = ?2",
                    params![place_of(tx, caller)?.repo.0, branch],
                )?;
                Ok(())
            },
        )?;
        Ok(MergeTask {
            owner: row.owner,
            worktree: row.worktree,
            target: row.target,
            head: row.head,
            since: row.since,
            run: row.run,
            from,
            merged: row.merged,
        })
    }

    pub fn merge_task(&self, caller: NodeId, branch: &str) -> Result<MergeTask, Refusal> {
        let row = load(&self.lock(), caller, branch)?
            .ok_or_else(|| Refusal::NoRound(branch.to_owned()))?;
        Ok(MergeTask {
            owner: row.owner,
            from: row.state,
            worktree: row.worktree,
            target: row.target,
            head: row.head,
            since: row.since,
            run: row.run,
            merged: row.merged,
        })
    }

    pub fn branch_task(
        &self,
        caller: NodeId,
        branch: &str,
    ) -> Result<Option<(String, String)>, Refusal> {
        let owner = match self.merge_task(caller, branch) {
            Ok(round) => round.owner,
            Err(Refusal::NoRound(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        let found = self.task_of(owner).and_then(|task| {
            task.map(|task| self.place_of(owner).map(|place| (place.worktree, task)))
                .transpose()
        });
        match found {
            Ok(found) => Ok(found),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn gate_merges(&self, caller: NodeId, target: &str) -> rusqlite::Result<Vec<String>> {
        let db = self.lock();
        let repo = place_of(&db, caller)?.repo;
        db.prepare(
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

    pub fn merge_end(
        &self,
        caller: NodeId,
        branch: &str,
        run: i64,
        ended: Ended,
        via: Option<&Via>,
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
            caller,
            branch,
            GateCall::MergeEnd(end),
            detail,
            via,
            |tx, row| {
                if row.run != run {
                    return Err(Refusal::StaleRun {
                        branch: branch.to_owned(),
                        run,
                    });
                }
                Ok(update(tx, caller, branch, row.state, None, merged)?)
            },
        )?;
        Ok(row.state)
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
        let row = self.apply(caller, branch, GateCall::Fail, detail, via, |tx, row| {
            Ok(update(tx, caller, branch, row.state, None, None)?)
        })?;
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
        let from = load(&db, caller, branch)?.map(|row| row.state);
        let entry = Entry::Refused {
            what: None,
            how: Kind::Queue,
            from: events::who(&db, caller)?,
            to: branch.to_owned(),
            reason: reason.clone(),
            via: via.cloned(),
            gate: Some(trace(&db, caller, verb, branch, from, None, Vec::new())?),
        };
        events::write(&db, &entry, None)
    }

    fn apply(
        &self,
        caller: NodeId,
        branch: &str,
        call: GateCall,
        detail: Detail,
        via: Option<&Via>,
        write: impl FnOnce(&Transaction, &Row) -> Result<(), Refusal>,
    ) -> Result<Row, Refusal> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let old = load(&tx, caller, branch)?;
        let from = old.as_ref().map_or(GateState::Working, |row| row.state);
        let gate = match (&old, protocol::sender(call.verb())) {
            (_, Sender::Owner) => None,
            (None, Sender::GateOrReviewer | Sender::Gate) => {
                return Err(Refusal::NoRound(branch.to_owned()));
            }
            (Some(row), Sender::Gate) => Some((row.gate, row.gate == caller)),
            (Some(row), Sender::GateOrReviewer) if row.owner == caller => {
                return Err(Refusal::OwnVerdict(branch.to_owned()));
            }
            (Some(row), Sender::GateOrReviewer) => Some((
                row.gate,
                row.gate == caller || parent_of(&tx, caller)? == Some(row.gate),
            )),
        };
        if let Some((gate, false)) = gate {
            return Err(Refusal::NotGate {
                branch: branch.to_owned(),
                gate: name_of(&tx, gate)?,
            });
        }
        let to = protocol::step(from, call)?;
        let row = match old {
            Some(row) => Row { state: to, ..row },
            None => Row {
                owner: caller,
                gate: caller,
                target: String::new(),
                worktree: String::new(),
                state: to,
                head: String::new(),
                since: 0,
                run: 0,
                merged: None,
            },
        };
        write(&tx, &row)?;
        let row = load(&tx, caller, branch)?.expect("방금 쓴 관문 줄");
        let trace = trace(
            &tx,
            caller,
            call.verb(),
            branch,
            Some(from),
            Some(to),
            detail.findings,
        )?;
        let what = protocol::gate_kind(
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
        );
        let mut sent = Vec::new();
        for receiver in protocol::receivers(call) {
            let to_node = match receiver {
                Receiver::Gate => row.gate,
                Receiver::Owner => row.owner,
                Receiver::Root if exists(&tx, row.owner)? => root_of(&tx, row.owner)?,
                Receiver::Root => continue,
            };
            if to_node == caller || sent.contains(&to_node) || !exists(&tx, to_node)? {
                continue;
            }
            let mut body = detail.lines.clone();
            body.push(protocol::next_step(call, *receiver, branch));
            let stamp = Stamp {
                via: via.cloned(),
                gate: Some(trace.clone()),
            };
            mailbox::enqueue(
                &tx,
                caller,
                Some(to_node),
                Kind::Queue,
                &what,
                &body.join("\n"),
                stamp,
            )?;
            sent.push(to_node);
        }
        if sent.is_empty() {
            let mut body = detail.lines;
            body.push(protocol::KEPT_FOR_CALLER.to_owned());
            let stamp = Stamp {
                via: via.cloned(),
                gate: Some(trace),
            };
            mailbox::enqueue(
                &tx,
                caller,
                Some(caller),
                Kind::Queue,
                &what,
                &body.join("\n"),
                stamp,
            )?;
            sent.push(caller);
        }
        tx.commit()?;
        for to_node in sent {
            self.publish_sent(&db, caller, to_node)?;
        }
        Ok(row)
    }
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

fn load(db: &Connection, caller: NodeId, branch: &str) -> rusqlite::Result<Option<Row>> {
    let repo = place_of(db, caller)?.repo;
    db.query_row(
        "SELECT owner, gate, target, worktree, state, head, opened, run, merged FROM gates WHERE repo = ?1 AND branch = ?2",
        params![repo.0, branch],
        |row| {
            Ok(Row {
                owner: NodeId(row.get(0)?),
                gate: NodeId(row.get(1)?),
                target: row.get(2)?,
                worktree: row.get(3)?,
                state: from_json(row, 4)?,
                head: row.get(5)?,
                since: row.get(6)?,
                run: row.get(7)?,
                merged: row.get(8)?,
            })
        },
    )
    .optional()
}

fn update(
    db: &Connection,
    caller: NodeId,
    branch: &str,
    state: GateState,
    verdict: Option<&str>,
    merged: Option<&str>,
) -> rusqlite::Result<()> {
    let repo = place_of(db, caller)?.repo;
    db.execute(
        "UPDATE gates SET state = ?3, verdict = coalesce(?4, verdict), merged = coalesce(?5, merged), updated = unixepoch()
         WHERE repo = ?1 AND branch = ?2",
        params![repo.0, branch, to_json(&state), verdict, merged],
    )?;
    Ok(())
}

fn trace(
    db: &Connection,
    caller: NodeId,
    verb: GateVerb,
    branch: &str,
    from: Option<GateState>,
    to: Option<GateState>,
    findings: Vec<Finding>,
) -> rusqlite::Result<GateTrace> {
    let (role, agent, model) = db.query_row(
        "SELECT role, agent, model FROM nodes WHERE id = ?1",
        [caller.0],
        |row| Ok((row.get(0)?, from_json(row, 1)?, row.get(2)?)),
    )?;
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
