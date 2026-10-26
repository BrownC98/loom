use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::events::{self, Entry, GateTrace, Via};
use super::mailbox::{self, Stamp};
use super::{Graph, NodeId, REACHABLE, from_json, name_of, parent_of, place_of, to_json};
use crate::protocol::{
    self, AgentId, Finding, GateCall, GateFacts, GateState, GateVerb, Judgment, Kind, MergeEnd,
    MessageKind, Peer, Receiver, Refusal, Sender, Target,
};
use crate::task;
use crate::worktree::RepoId;

const MIN_HEAD: usize = 7;

pub struct Round<'a> {
    pub branch: &'a str,
    pub target: &'a str,
    pub worktree: &'a str,
    pub head: &'a str,
}

#[derive(Debug, PartialEq)]
pub enum GateChoice {
    Parent { gate: NodeId, worktree: String },
    Owner,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Queued {
    pub branch: String,
    pub owner: NodeId,
    pub head: String,
    pub worktree: String,
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

pub struct Reclaim {
    pub owner: NodeId,
    pub gate: NodeId,
    pub target: String,
    pub worktree: String,
    pub head: String,
    pub reviewers: Vec<NodeId>,
}

pub struct Merge<'a> {
    pub sha: &'a str,
    pub target: &'a str,
}

pub struct Ended<'a> {
    pub end: MergeEnd,
    pub merged: Option<Merge<'a>>,
    pub cause: Option<String>,
    pub log: Option<String>,
    pub lines: Vec<String>,
    pub findings: Vec<Finding>,
}

struct Row {
    owner: NodeId,
    gate: NodeId,
    worktree: String,
    state: GateState,
    head: String,
}

impl Row {
    fn target_kind(&self) -> Target {
        match self.gate == self.owner {
            true => Target::Base,
            false => Target::Coordinator,
        }
    }
}

#[derive(Default)]
struct Detail {
    target: String,
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
        let found = self.lock().query_row(
            &format!(
                "SELECT {}, {}",
                parent_gate("?1", "id"),
                parent_gate("?1", "worktree")
            ),
            [owner.0],
            |r| Ok((r.get::<_, Option<u32>>(0)?, r.get::<_, Option<String>>(1)?)),
        )?;
        Ok(match found {
            (Some(gate), Some(worktree)) => GateChoice::Parent {
                gate: NodeId(gate),
                worktree,
            },
            _ => GateChoice::Owner,
        })
    }

    pub fn ready(
        &self,
        owner: NodeId,
        round: &Round,
        via: Option<&Via>,
    ) -> Result<String, Refusal> {
        let detail = Detail {
            target: round.target.to_owned(),
            ..Detail::default()
        };
        let repo = place_of(&self.lock(), owner)?.repo;
        let row = self.apply(
            Caller::Node(owner),
            repo,
            round.branch,
            GateCall::Ready,
            detail,
            via,
            |tx, row| {
                if let Some(open) = load(tx, repo, round.branch)?
                    && matches!(open.state, GateState::Ready | GateState::Passed)
                    && open.head == round.head
                {
                    return Err(Refusal::SameHead(round.branch.to_owned()));
                }
                tx.execute(
                    "INSERT INTO gates (repo, branch, owner, worktree, state, head, opened)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, unixepoch())
                     ON CONFLICT (repo, branch) DO UPDATE SET owner = ?3, worktree = ?4,
                        state = ?5, head = ?6, queued = NULL, seq = NULL, batch = NULL, opened = unixepoch()",
                    params![
                        repo.0,
                        round.branch,
                        owner.0,
                        round.worktree,
                        to_json(&row.state),
                        round.head
                    ],
                )?;
                tx.execute(
                    "UPDATE gate_votes SET vote = NULL WHERE repo = ?1 AND branch = ?2",
                    params![repo.0, round.branch],
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
        head: &str,
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
                if head.len() < MIN_HEAD || !row.head.starts_with(head) {
                    return Err(Refusal::StaleHead {
                        branch: branch.to_owned(),
                        head: row.head.clone(),
                    });
                }
                update(tx, repo, branch, row.state)?;
                tx.execute(
                    "INSERT INTO gate_votes (repo, branch, reviewer, vote, head, findings) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT (repo, branch, reviewer) DO UPDATE SET vote = ?4, head = ?5, findings = ?6",
                    params![repo.0, branch, caller.0, to_json(&judgment), row.head, findings],
                )?;
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
            .prepare("SELECT sha FROM gate_merges WHERE repo = ?1 AND target = ?2")?
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

    pub fn queued_gates(&self) -> rusqlite::Result<Vec<(RepoId, Option<String>)>> {
        self.lock()
            .prepare(&format!(
                "SELECT repo, {} AS into_worktree FROM gates WHERE {} GROUP BY repo, into_worktree ORDER BY min(seq)",
                parent_gate("gates.owner", "worktree"),
                settled()
            ))?
            .query_map([], |r| Ok((RepoId(r.get(0)?), r.get(1)?)))?
            .collect()
    }

    pub fn batch_open(
        &self,
        repo: RepoId,
        gate_worktree: Option<&str>,
        target: &str,
    ) -> rusqlite::Result<Option<(BatchId, Vec<Queued>)>> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let queue: Vec<Queued> = tx
            .prepare(&format!(
                "{QUEUED} WHERE repo = ?1 AND {} IS ?2 AND {} ORDER BY seq",
                parent_gate("gates.owner", "worktree"),
                settled()
            ))?
            .query_map(params![repo.0, gate_worktree], read_queued)?
            .collect::<rusqlite::Result<_>>()?;
        if queue.is_empty() {
            return Ok(None);
        }
        tx.execute(
            "INSERT INTO batches (repo, target) VALUES (?1, ?2)",
            params![repo.0, target],
        )?;
        let id = BatchId(tx.last_insert_rowid());
        for member in &queue {
            tx.execute(
                "UPDATE gates SET batch = ?3 WHERE repo = ?1 AND branch = ?2",
                params![repo.0, member.branch, id.0],
            )?;
        }
        tx.commit()?;
        Ok(Some((id, queue)))
    }

    pub fn batch_members(&self, id: BatchId) -> rusqlite::Result<Vec<(String, Option<String>)>> {
        self.lock()
            .prepare(&format!(
                "SELECT branch, {} FROM gates WHERE batch = ?1",
                parent_gate("gates.owner", "worktree")
            ))?
            .query_map([id.0], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect()
    }

    pub fn waiting_rounds(&self, owner: NodeId) -> rusqlite::Result<Vec<(RepoId, String)>> {
        self.lock()
            .prepare(
                "SELECT repo, branch FROM gates WHERE owner = ?1 AND state IN (?2, ?3) AND batch IS NULL ORDER BY rowid",
            )?
            .query_map(
                params![
                    owner.0,
                    to_json(&GateState::Ready),
                    to_json(&GateState::Passed)
                ],
                |r| Ok((RepoId(r.get(0)?), r.get(1)?)),
            )?
            .collect()
    }

    pub fn tell_gate(&self, repo: RepoId, branch: &str, target: &str) -> rusqlite::Result<()> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let Some(row) = load(&tx, repo, branch)? else {
            return Ok(());
        };
        if !matches!(row.state, GateState::Ready | GateState::Passed) || !exists(&tx, row.gate)? {
            return Ok(());
        }
        let what = MessageKind::Ready {
            branch: branch.to_owned(),
            head: row.head.clone(),
            worktree: row.worktree.clone(),
            target: target.to_owned(),
        };
        let trace = trace(
            &tx,
            Caller::Loom,
            GateVerb::Ready,
            branch,
            Some(row.state),
            Some(row.state),
            Vec::new(),
        )?;
        let body = protocol::next_step(
            GateCall::Ready,
            Receiver::Gate,
            row.target_kind(),
            branch,
            &row.head,
        );
        post(
            &tx,
            Caller::Loom,
            row.gate,
            Kind::Queue,
            &what,
            &body,
            None,
            &trace,
        )?;
        tx.commit()?;
        self.publish_nodes();
        self.mail.notify_waiters();
        Ok(())
    }

    pub fn open_round(&self, repo: RepoId, branch: &str) -> rusqlite::Result<bool> {
        self.lock().query_row(
            "SELECT EXISTS(SELECT 1 FROM gates WHERE repo = ?1 AND branch = ?2
             AND (state IN (?3, ?4) OR batch IS NOT NULL))",
            params![
                repo.0,
                branch,
                to_json(&GateState::Ready),
                to_json(&GateState::Passed)
            ],
            |r| r.get(0),
        )
    }

    pub fn target_moves(&self, repo: RepoId, target: &str) -> rusqlite::Result<u32> {
        self.lock().query_row(
            "SELECT count(*) FROM batches WHERE repo = ?1 AND target = ?2 AND moved = 1",
            params![repo.0, target],
            |r| r.get(0),
        )
    }

    pub fn batch_moved(&self, id: BatchId) -> rusqlite::Result<()> {
        self.lock()
            .execute("UPDATE batches SET moved = 1 WHERE id = ?1", [id.0])?;
        Ok(())
    }

    pub fn batch_checked(
        &self,
        id: BatchId,
        base: &str,
        tip: &str,
        members: &[&str],
    ) -> rusqlite::Result<bool> {
        let members = serde_json::to_string(members).expect("문자열 목록은 늘 직렬화된다");
        let written = self.lock().execute(
            "UPDATE batches SET base = ?2, tip = ?3 WHERE id = ?1
             AND (SELECT count(*) FROM gates WHERE batch = ?1) = json_array_length(?4)
             AND NOT EXISTS (SELECT 1 FROM json_each(?4) m
                 WHERE NOT EXISTS (SELECT 1 FROM gates WHERE batch = ?1 AND branch = m.value))",
            params![id.0, base, tip, members],
        )?;
        Ok(written == 1)
    }

    pub fn batch_ended(&self, id: BatchId) -> rusqlite::Result<()> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        tx.execute(
            "DELETE FROM batches WHERE moved = 1
             AND NOT EXISTS (SELECT 1 FROM gates WHERE batch = batches.id)
             AND (repo, target) = (SELECT repo, target FROM batches WHERE id = ?1 AND moved = 0
                 AND NOT EXISTS (SELECT 1 FROM gates WHERE batch = ?1))",
            [id.0],
        )?;
        close(&tx, id)?;
        tx.commit()
    }

    pub fn batch_close(&self, id: BatchId) -> rusqlite::Result<()> {
        close(&self.lock(), id)
    }

    pub fn stale_batches(&self) -> rusqlite::Result<Vec<Stale>> {
        let db = self.lock();
        let mut stale: Vec<Stale> = db
            .prepare(
                "SELECT id, repo, target, base, tip FROM batches b
                 WHERE moved = 0 OR EXISTS (SELECT 1 FROM gates WHERE batch = b.id) ORDER BY id",
            )?
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
                .prepare(&format!("{QUEUED} WHERE batch = ?1 ORDER BY seq"))?
                .query_map([batch.id.0], read_queued)?
                .collect::<rusqlite::Result<_>>()?;
        }
        Ok(stale)
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
                update(tx, repo, branch, row.state)?;
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
            sha: merged.as_ref().map(|m| m.sha.to_owned()),
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
                update(tx, repo, branch, row.state)?;
                tx.execute(
                    "UPDATE gates SET batch = NULL, queued = NULL, seq = NULL WHERE repo = ?1 AND branch = ?2",
                    params![repo.0, branch],
                )?;
                if let Some(merged) = &merged {
                    tx.execute(
                        "INSERT OR IGNORE INTO gate_merges (repo, target, sha) VALUES (?1, ?2, ?3)",
                        params![repo.0, merged.target, merged.sha],
                    )?;
                    tx.execute(
                        "UPDATE gates SET merged_into = ?3 WHERE repo = ?1 AND branch = ?2",
                        params![repo.0, branch, merged.target],
                    )?;
                }
                Ok(())
            },
        )?;
        Ok(row.state)
    }

    pub fn reclaim_rounds(&self) -> rusqlite::Result<Vec<(RepoId, String, bool)>> {
        self.lock()
            .prepare(&format!(
                "SELECT repo, branch, EXISTS
                    (SELECT 1 FROM nodes o WHERE o.id = gates.owner AND o.parent IS NOT NULL
                     AND NOT EXISTS (SELECT 1 FROM nodes c WHERE c.parent = o.id))
                 OR EXISTS (SELECT 1 FROM nodes n WHERE {}) FROM gates WHERE state = ?1 ORDER BY rowid",
                of_round(&gate_of("gates.owner"), "gates.owner", "gates.branch")
            ))?
            .query_map([to_json(&GateState::Merged)], |r| {
                Ok((RepoId(r.get(0)?), r.get(1)?, r.get(2)?))
            })?
            .collect()
    }

    pub fn reclaim(&self, repo: RepoId, branch: &str) -> rusqlite::Result<Option<Reclaim>> {
        let db = self.lock();
        let found = db
            .query_row(
                &format!(
                    "SELECT owner, {}, merged_into, worktree, head FROM gates
                     WHERE repo = ?1 AND branch = ?2 AND state = ?3 AND merged_into IS NOT NULL",
                    gate_of("gates.owner")
                ),
                params![repo.0, branch, to_json(&GateState::Merged)],
                |r| {
                    Ok(Reclaim {
                        owner: NodeId(r.get(0)?),
                        gate: NodeId(r.get(1)?),
                        target: r.get(2)?,
                        worktree: r.get(3)?,
                        head: r.get(4)?,
                        reviewers: Vec::new(),
                    })
                },
            )
            .optional()?;
        let Some(mut found) = found else {
            return Ok(None);
        };
        found.reviewers = db
            .prepare(&format!(
                "SELECT n.id FROM nodes n, gates WHERE gates.repo = ?1 AND gates.branch = ?2 AND {} ORDER BY n.id",
                of_round(&gate_of("gates.owner"), "gates.owner", "gates.branch")
            ))?
            .query_map(params![repo.0, branch], |r| Ok(NodeId(r.get(0)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Some(found))
    }

    pub fn note_reclaim(&self, round: &Reclaim, body: &str) -> rusqlite::Result<()> {
        let db = self.lock();
        let to = parent_of(&db, round.owner)
            .ok()
            .flatten()
            .unwrap_or(round.gate);
        let told: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE recipient = ?1 AND sender IS NULL AND body = ?2)",
            params![to.0, body],
            |r| r.get(0),
        )?;
        if told || !exists(&db, to)? {
            return Ok(());
        }
        mailbox::enqueue_from_loom(&db, to, Kind::Queue, &MessageKind::Note, body, None)?;
        self.publish_nodes();
        self.mail.notify_waiters();
        Ok(())
    }

    pub fn introduce_reviewer(&self, node: NodeId) -> rusqlite::Result<()> {
        let db = self.lock();
        let Some((gate, branch)) = db
            .query_row(
                "SELECT parent, review FROM nodes WHERE id = ?1 AND review IS NOT NULL",
                [node.0],
                |r| Ok((NodeId(r.get(0)?), r.get::<_, String>(1)?)),
            )
            .optional()?
        else {
            return Ok(());
        };
        let seated: Vec<(NodeId, String, Option<AgentId>)> = db
            .prepare(&format!(
                "SELECT n.id, n.name, n.agent FROM nodes n
                 WHERE n.parent = ?1 AND n.review = ?2 AND {REACHABLE} ORDER BY n.id"
            ))?
            .query_map(params![gate.0, branch], |r| {
                Ok((NodeId(r.get(0)?), r.get(1)?, from_json(r, 2)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let head = load(&db, place_of(&db, gate)?.repo, &branch)?.map(|row| row.head);
        let names: Vec<(NodeId, String)> = seated
            .iter()
            .map(|(id, name, _)| (*id, name.clone()))
            .collect();
        let newcomer: Vec<(NodeId, String)> = names
            .iter()
            .filter(|(id, _)| *id == node)
            .cloned()
            .collect();
        for (id, _, agent) in &seated {
            let (peers, kind) = match *id == node {
                true => (peers_of(&db, &names, node, &branch)?, Kind::Queue),
                false => (
                    peers_of(&db, &newcomer, *id, &branch)?,
                    steer_kind(self.linked, agent),
                ),
            };
            let mut body = Vec::new();
            if let (true, Some(head)) = (*id == node, &head) {
                body.push(protocol::first_round_step(&branch, head));
            }
            if !peers.is_empty() {
                body.push(protocol::peers_step(&branch, &peers));
            }
            if body.is_empty() {
                continue;
            }
            let body = body.join("\n");
            mailbox::enqueue_from_loom(&db, *id, kind, &MessageKind::Note, &body, None)?;
            self.publish_nodes();
        }
        self.mail.notify_waiters();
        Ok(())
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
                worktree: String::new(),
                state: to,
                head: String::new(),
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
                target: detail.target,
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
            let held = match call {
                GateCall::Verdict(Judgment::Pass) => pending(&tx, caller, &row, repo, branch)?,
                _ => Vec::new(),
            };
            let target = row.target_kind();
            for receiver in protocol::receivers(call, target) {
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
                body.push(match held.is_empty() {
                    true => protocol::next_step(call, *receiver, target, branch, &row.head),
                    false => protocol::held_step(*receiver, &held),
                });
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
            if call == GateCall::Ready {
                let replaced = matches!(from, GateState::Ready | GateState::Passed);
                let with_agents = reviewers(&tx, &row, branch, caller)?;
                let seated: Vec<(NodeId, String)> = with_agents
                    .iter()
                    .map(|(id, name, _)| (*id, name.clone()))
                    .collect();
                for (reviewer, name, agent) in &with_agents {
                    let judged = last_judged(&tx, *reviewer, branch)?;
                    let mut body = Vec::new();
                    let peers = peers_of(&tx, &seated, *reviewer, branch)?;
                    if !peers.is_empty() {
                        body.push(protocol::peers_step(branch, &peers));
                    }
                    body.push(protocol::review_again_step(
                        branch,
                        &row.head,
                        judged.as_deref(),
                        &findings_of(&tx, *reviewer, name, branch)?,
                    ));
                    let kind = match replaced {
                        true => steer_kind(self.linked, agent),
                        false => Kind::Queue,
                    };
                    post(
                        &tx,
                        caller,
                        *reviewer,
                        kind,
                        &what,
                        &body.join("\n"),
                        via,
                        &trace,
                    )?;
                    sent.push(*reviewer);
                }
            }
        }
        if withdrawn(from, to) {
            sent.extend(withdraw(&tx, caller, &row, branch, &trace, self.linked)?);
        }
        tx.commit()?;
        for to_node in sent {
            match caller {
                Caller::Node(me) => self.publish_sent(&db, me, to_node)?,
                Caller::Loom => self.publish_nodes(),
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
        (Caller::Node(me), Some(row), Sender::Reviewer)
            if row.target_kind() == Target::Coordinator =>
        {
            match row.gate == me {
                true => Ok(()),
                false => Err(accepts(db, row, branch)?),
            }
        }
        (Caller::Node(me), Some(row), Sender::Reviewer) => match parent_of(db, me)? {
            Some(parent) if parent == row.gate => match review_of(db, me)? {
                Some(review) if review == branch => Ok(()),
                review => Err(Refusal::NotReviewer {
                    branch: branch.to_owned(),
                    review,
                }),
            },
            _ => not_gate(row),
        },
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
    linked: fn(&AgentId) -> bool,
) -> rusqlite::Result<Vec<NodeId>> {
    let reviewers: Vec<(NodeId, String, Option<AgentId>)> = tx
        .prepare(&format!(
            "SELECT n.id, n.name, n.agent FROM nodes n WHERE {} AND n.id != ?3 ORDER BY n.id",
            in_round("?1", "?2", "?4")
        ))?
        .query_map(
            params![
                row.gate.0,
                row.owner.0,
                caller.node().map_or(0, |n| n.0),
                branch
            ],
            |r| Ok((NodeId(r.get(0)?), r.get(1)?, from_json(r, 2)?)),
        )?
        .collect::<rusqlite::Result<_>>()?;
    let what = MessageKind::Withdrawn {
        branch: branch.to_owned(),
        head: row.head.clone(),
    };
    for (node, name, agent) in &reviewers {
        let body =
            protocol::withdrawn_step(branch, row.state, &findings_of(tx, *node, name, branch)?);
        mailbox::enqueue_from_loom(
            tx,
            *node,
            steer_kind(linked, agent),
            &what,
            &body,
            Some(trace.clone()),
        )?;
    }
    Ok(reviewers.into_iter().map(|(node, ..)| node).collect())
}

fn steer_kind(linked: fn(&AgentId) -> bool, agent: &Option<AgentId>) -> Kind {
    match agent.as_ref().is_some_and(linked) {
        true => Kind::Steer,
        false => Kind::Queue,
    }
}

fn reviewers(
    tx: &Transaction,
    row: &Row,
    branch: &str,
    caller: Caller,
) -> rusqlite::Result<Vec<(NodeId, String, Option<AgentId>)>> {
    tx.prepare(&format!(
        "SELECT n.id, n.name, n.agent FROM nodes n WHERE {} AND n.id != ?4 ORDER BY n.id",
        in_round("?1", "?2", "?3")
    ))?
    .query_map(
        params![
            row.gate.0,
            row.owner.0,
            branch,
            caller.node().map_or(0, |n| n.0)
        ],
        |r| Ok((NodeId(r.get(0)?), r.get(1)?, from_json(r, 2)?)),
    )?
    .collect()
}

fn peers_of(
    db: &Connection,
    seated: &[(NodeId, String)],
    me: NodeId,
    branch: &str,
) -> rusqlite::Result<Vec<Peer>> {
    seated
        .iter()
        .filter(|(id, _)| *id != me)
        .map(|(id, name)| {
            Ok(Peer {
                name: name.clone(),
                findings: findings_of(db, *id, name, branch)?,
            })
        })
        .collect()
}

fn findings_of(
    db: &Connection,
    reviewer: NodeId,
    name: &str,
    branch: &str,
) -> rusqlite::Result<String> {
    let given = last_verdict(db, reviewer, branch, "findings")?;
    Ok(given.unwrap_or_else(|| task::findings_in_task(branch, name)))
}

fn last_judged(
    db: &Connection,
    reviewer: NodeId,
    branch: &str,
) -> rusqlite::Result<Option<String>> {
    last_verdict(db, reviewer, branch, "head")
}

fn last_verdict(
    db: &Connection,
    reviewer: NodeId,
    branch: &str,
    column: &str,
) -> rusqlite::Result<Option<String>> {
    db.query_row(
        &format!("SELECT {column} FROM gate_votes WHERE reviewer = ?1 AND branch = ?2"),
        params![reviewer.0, branch],
        |r| r.get(0),
    )
    .optional()
    .map(Option::flatten)
}

fn pending(
    tx: &Transaction,
    caller: Caller,
    row: &Row,
    repo: RepoId,
    branch: &str,
) -> rusqlite::Result<Vec<String>> {
    tx.prepare(&format!(
        "SELECT n.name FROM nodes n WHERE {} AND NOT {} AND n.id != ?4 ORDER BY n.id",
        in_round("?1", "?2", "?3"),
        voted("?5", "?3")
    ))?
    .query_map(
        params![
            row.gate.0,
            row.owner.0,
            branch,
            caller.node().map_or(0, |n| n.0),
            repo.0
        ],
        |r| r.get(0),
    )?
    .collect()
}

fn in_round(gate: &str, owner: &str, branch: &str) -> String {
    format!("({} AND {REACHABLE})", of_round(gate, owner, branch))
}

fn of_round(gate: &str, owner: &str, branch: &str) -> String {
    format!("(n.parent = {gate} AND n.id != {owner} AND n.review = {branch})")
}

pub(super) fn check_review(
    db: &Connection,
    parent: NodeId,
    repo: RepoId,
    branch: &str,
    role: &str,
) -> Result<(), Refusal> {
    match load(db, repo, branch)? {
        None => Err(Refusal::NoRound(branch.to_owned())),
        Some(row) if row.gate != parent => Err(Refusal::NotGate {
            branch: branch.to_owned(),
            gate: name_of(db, row.gate)?,
        }),
        Some(row) if row.target_kind() == Target::Coordinator => Err(accepts(db, &row, branch)?),
        Some(_) => match seated(db, parent, branch, role)? {
            Some(reviewer) => Err(Refusal::ReviewerSeated {
                branch: branch.to_owned(),
                role: role.to_owned(),
                reviewer,
            }),
            None => release(db, repo, branch),
        },
    }
}

fn close(db: &Connection, id: BatchId) -> rusqlite::Result<()> {
    db.execute(
        "DELETE FROM batches WHERE id = ?1 AND moved = 0
         AND NOT EXISTS (SELECT 1 FROM gates WHERE batch = ?1)",
        [id.0],
    )?;
    Ok(())
}

pub(super) fn close_owned(
    tx: &Transaction,
    owner: NodeId,
    linked: fn(&AgentId) -> bool,
) -> Result<Vec<NodeId>, Refusal> {
    let open: Vec<(RepoId, String)> = tx
        .prepare("SELECT repo, branch FROM gates WHERE owner = ?1 AND state IN (?2, ?3)")?
        .query_map(
            params![
                owner.0,
                to_json(&GateState::Ready),
                to_json(&GateState::Passed)
            ],
            |r| Ok((RepoId(r.get(0)?), r.get(1)?)),
        )?
        .collect::<rusqlite::Result<_>>()?;
    let mut sent = Vec::new();
    for (repo, branch) in open {
        let row = load(tx, repo, &branch)?.expect("방금 찾은 관문 줄");
        release(tx, repo, &branch)?;
        update(tx, repo, &branch, GateState::Working)?;
        tx.execute(
            "UPDATE gates SET queued = NULL, seq = NULL WHERE repo = ?1 AND branch = ?2",
            params![repo.0, branch],
        )?;
        tx.execute(
            "UPDATE gate_votes SET vote = NULL WHERE repo = ?1 AND branch = ?2",
            params![repo.0, branch],
        )?;
        let trace = trace(
            tx,
            Caller::Loom,
            GateVerb::Queue,
            &branch,
            Some(row.state),
            Some(GateState::Working),
            Vec::new(),
        )?;
        sent.extend(withdraw(tx, Caller::Loom, &row, &branch, &trace, linked)?);
    }
    Ok(sent)
}

pub(super) struct OpenGate {
    repo: RepoId,
    branch: String,
    gate: u32,
}

pub(super) fn open_gates(db: &Connection, owners: &[NodeId]) -> rusqlite::Result<Vec<OpenGate>> {
    let owners = serde_json::to_string(&owners.iter().map(|n| n.0).collect::<Vec<_>>())
        .expect("번호 목록은 늘 직렬화된다");
    db.prepare(&format!(
        "SELECT repo, branch, {} FROM gates WHERE owner IN (SELECT value FROM json_each(?1))
         AND state IN (?2, ?3)",
        gate_of("gates.owner")
    ))?
    .query_map(
        params![
            owners,
            to_json(&GateState::Ready),
            to_json(&GateState::Passed)
        ],
        |r| {
            Ok(OpenGate {
                repo: RepoId(r.get(0)?),
                branch: r.get(1)?,
                gate: r.get(2)?,
            })
        },
    )?
    .collect()
}

pub(super) fn regate(db: &Connection, before: Vec<OpenGate>) -> Result<(), Refusal> {
    for open in before {
        let now: u32 = db.query_row(
            &format!(
                "SELECT {} FROM gates WHERE repo = ?1 AND branch = ?2",
                gate_of("gates.owner")
            ),
            params![open.repo.0, open.branch],
            |r| r.get(0),
        )?;
        if now == open.gate {
            continue;
        }
        release(db, open.repo, &open.branch)?;
        db.execute(
            "UPDATE gate_votes SET vote = NULL WHERE repo = ?1 AND branch = ?2",
            params![open.repo.0, open.branch],
        )?;
        db.execute(
            "UPDATE gates SET state = ?3, queued = NULL, seq = NULL
             WHERE repo = ?1 AND branch = ?2 AND state = ?4",
            params![
                open.repo.0,
                open.branch,
                to_json(&GateState::Ready),
                to_json(&GateState::Passed)
            ],
        )?;
    }
    Ok(())
}

fn release(db: &Connection, repo: RepoId, branch: &str) -> Result<(), Refusal> {
    let checked: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM gates g JOIN batches b ON b.id = g.batch
         WHERE g.repo = ?1 AND g.branch = ?2 AND b.tip IS NOT NULL)",
        params![repo.0, branch],
        |r| r.get(0),
    )?;
    if checked {
        return Err(Refusal::Publishing(branch.to_owned()));
    }
    db.execute(
        "UPDATE gates SET batch = NULL WHERE repo = ?1 AND branch = ?2",
        params![repo.0, branch],
    )?;
    Ok(())
}

fn seated(
    db: &Connection,
    gate: NodeId,
    branch: &str,
    role: &str,
) -> rusqlite::Result<Option<String>> {
    db.query_row(
        "SELECT name FROM nodes WHERE parent = ?1 AND review = ?2 AND role = ?3 ORDER BY id",
        params![gate.0, branch, role],
        |row| row.get(0),
    )
    .optional()
}

fn accepts(db: &Connection, row: &Row, branch: &str) -> rusqlite::Result<Refusal> {
    Ok(Refusal::CoordinatorAccepts {
        branch: branch.to_owned(),
        gate: name_of(db, row.gate)?,
    })
}

fn review_of(db: &Connection, node: NodeId) -> rusqlite::Result<Option<String>> {
    db.query_row("SELECT review FROM nodes WHERE id = ?1", [node.0], |row| {
        row.get(0)
    })
}

fn voted(repo: &str, branch: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM gate_votes v WHERE v.repo = {repo} AND v.branch = {branch}
            AND v.reviewer = n.id AND v.vote IS NOT NULL)"
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

fn exists(db: &Connection, node: NodeId) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM nodes WHERE id = ?1)",
        [node.0],
        |row| row.get(0),
    )
}

fn settled() -> String {
    format!(
        "state = '\"passed\"' AND batch IS NULL AND NOT EXISTS (SELECT 1 FROM nodes n WHERE {} AND NOT {})",
        in_round(&gate_of("gates.owner"), "gates.owner", "gates.branch"),
        voted("gates.repo", "gates.branch")
    )
}

fn parent_gate(owner: &str, column: &str) -> String {
    format!(
        "(SELECT p.{column} FROM nodes o JOIN nodes p ON p.id = o.parent JOIN repos r ON r.id = p.repo
          WHERE o.id = {owner} AND p.repo = o.repo AND p.worktree != r.path)"
    )
}

fn gate_of(owner: &str) -> String {
    format!("coalesce({}, {owner})", parent_gate(owner, "id"))
}

const QUEUED: &str = "SELECT branch, owner, head, opened, max(unixepoch() - coalesce(queued, unixepoch()), 0), worktree FROM gates";

fn read_queued(r: &rusqlite::Row) -> rusqlite::Result<Queued> {
    Ok(Queued {
        branch: r.get(0)?,
        owner: NodeId(r.get(1)?),
        head: r.get(2)?,
        worktree: r.get(5)?,
        since: r.get(3)?,
        waited: r.get::<_, i64>(4)?.unsigned_abs(),
    })
}

fn load(db: &Connection, repo: RepoId, branch: &str) -> rusqlite::Result<Option<Row>> {
    db.query_row(
        &format!(
            "SELECT owner, {}, worktree, CASE WHEN batch IS NULL THEN state ELSE '\"merging\"' END, head
             FROM gates WHERE repo = ?1 AND branch = ?2",
            gate_of("gates.owner")
        ),
        params![repo.0, branch],
        |row| {
            Ok(Row {
                owner: NodeId(row.get(0)?),
                gate: NodeId(row.get(1)?),
                worktree: row.get(2)?,
                state: from_json(row, 3)?,
                head: row.get(4)?,
            })
        },
    )
    .optional()
}

fn update(db: &Connection, repo: RepoId, branch: &str, state: GateState) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE gates SET state = ?3 WHERE repo = ?1 AND branch = ?2",
        params![repo.0, branch, to_json(&state)],
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
