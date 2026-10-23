//! 병합 관문 상태: 저장소·브랜치마다 한 줄(`gates`)과 도는 병합 묶음(`batches`). 전이는 `protocol::step`만 정한다.
//! 이 모듈은 줄을 읽고 쓰고, 부를 수 있는 노드를 보고, 받는 노드에 고정 꼴 메시지를 넣고, 같은 트랜잭션에서 흔적을 남긴다.
//! git으로 아는 사실(담당 워크트리의 브랜치, 미커밋 변경, 포함 관계, 커밋)은 데몬이 읽어 넘긴다.
//! 근거: 출력 표준 R8 v5 ④·⑤(2026-10-23 사람 결정 Q559 A). 관문 고르기는 Q692 A, 병합 큐는 Q697 A다.

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use super::events::{self, Entry, GateTrace, Via, Who};
use super::mailbox::{self, Stamp};
use super::{Graph, Kind, NodeId, Refusal, from_json, name_of, parent_of, place_of, to_json};
use crate::profile::AgentId;
use crate::protocol::{
    self, COORDINATOR_ROLE, Finding, GateCall, GateFacts, GateState, GateVerb, Judgment, MergeEnd,
    MessageKind, Receiver, Sender, Target,
};
use crate::worktree::RepoId;

/// `loom ready`이 데몬에서 읽은 브랜치 사실.
pub struct Round<'a> {
    pub branch: &'a str,
    pub target: &'a str,
    /// 담당 워크트리(데몬이 보는 경로).
    pub worktree: &'a str,
    pub head: &'a str,
}

/// 브랜치 담당의 관문(2026-10-23 사람 결정 Q692 A).
#[derive(Debug, PartialEq)]
pub enum GateChoice {
    /// 부모가 코디다. 대상은 코디 워크트리의 브랜치다.
    Coordinator { gate: NodeId, worktree: String },
    /// 그 밖(코디, 총괄이 띄운 워커): 담당 자신이 관문이다. 대상은 원래 체크아웃의 브랜치다.
    Owner,
}

/// 병합 큐에 선 브랜치 하나. 큐 순서(pass 판정 순)로 읽는다.
#[derive(Clone, Debug, PartialEq)]
pub struct Queued {
    pub branch: String,
    pub owner: NodeId,
    /// 검토한 커밋. 묶음은 이 커밋만 쌓는다.
    pub head: String,
    /// audit 시작 시각: 이번 차례의 `loom ready`.
    pub since: i64,
    /// pass 판정부터 지금까지 기다린 초.
    pub waited: u64,
}

/// 묶음 번호(`batches.id`). 끝난 묶음의 번호는 다시 쓰지 않는다.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BatchId(pub i64);

/// 데몬이 끝내지 못한 묶음. 다시 띄운 데몬이 거둔다.
#[derive(Debug, PartialEq)]
pub struct Stale {
    pub id: BatchId,
    pub repo: RepoId,
    pub target: String,
    pub base: Option<String>,
    /// 검사를 지난 묶음 끝. 없으면 검사 전에 끝났다.
    pub tip: Option<String>,
    /// 아직 `Merging`인 브랜치(쌓은 순서).
    pub members: Vec<Queued>,
}

/// 병합 큐가 브랜치 하나를 끝낸 꼴과 알릴 값.
pub struct Ended<'a> {
    pub end: MergeEnd,
    /// 병합 커밋. 병합했을 때만 있다.
    pub merged: Option<&'a str>,
    pub cause: Option<String>,
    /// 묶음 검사 기록 파일(배포판 안의 경로).
    pub log: Option<String>,
    /// 받는 노드에 덧붙일 본문 줄(묶음 번호, 배포 결과).
    pub lines: Vec<String>,
    pub findings: Vec<Finding>,
}

/// 마지막 판정. 관문 줄에 serde JSON으로 둔다.
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
    /// 스스로 관문인 담당의 대상은 원래 체크아웃의 브랜치, 코디가 관문이면 코디 브랜치다(`GateChoice`).
    fn target_kind(&self) -> Target {
        match self.gate == self.owner {
            true => Target::Base,
            false => Target::Coordinator,
        }
    }
}

/// 관문 호출 하나가 받는 노드에 보내는 칸(`protocol::GateFacts`의 호출 쪽 값)과 본문 줄, 흔적에 쓸 audit 결과.
/// 브랜치·커밋·워크트리·대상·검토 모델은 관문 줄과 부른 노드에서 채운다.
#[derive(Default)]
struct Detail {
    findings_file: Option<String>,
    sha: Option<String>,
    cause: Option<String>,
    log: Option<String>,
    lines: Vec<String>,
    findings: Vec<Finding>,
}

/// 관문 호출을 부른 쪽. 병합 큐(데몬)는 노드가 아니다.
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
    /// 담당 `owner`의 관문. 부모가 코디면 코디, 아니면 담당 자신이다.
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

    /// `loom ready`: 담당이 브랜치를 관문에 보낸다. 데몬이 담당·미커밋을 이미 봤다. 돌려준 값은 관문 이름이다.
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

    /// 관문 스크립트가 없는 저장소의 `loom ready`: 관문 상태 없이 관문에 `ready`만 보낸다. 관문은 손으로 병합하고 `loom done`으로 알린다.
    /// 근거: 요청 #3067(ktasearch: 워커 브랜치가 아무도 모르게 1.5시간 기다렸다).
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

    /// 부모가 코디인가. 코디 밑 워커의 브랜치는 코디가 받는다.
    pub fn under_coordinator(&self, node: NodeId) -> rusqlite::Result<bool> {
        Ok(coordinator_parent(&self.lock(), node)?.is_some())
    }

    /// `owner`가 이 브랜치의 이 커밋으로 `ready`를 보냈는가(관문 상태가 있든 없든). `loom done`의 담당 브랜치 경고가 본다.
    pub fn ready_sent(&self, owner: NodeId, branch: &str, head: &str) -> rusqlite::Result<bool> {
        self.lock().query_row(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE sender = ?1 AND what ->> 'kind' = 'ready'
             AND what ->> 'branch' = ?2 AND what ->> 'head' = ?3)",
            params![owner.0, branch, head],
            |row| row.get(0),
        )
    }

    /// `loom verdict`: 관문이 띄운 검토자가 판정한다. block은 결함 파일이 있어야 한다. pass는 브랜치를 병합 큐에 세운다.
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
                // 둘째 pass는 큐 자리를 바꾸지 않는다. 큐 순서는 `seq`다: 초 단위 `queued`는 같은 초의 두 pass를 가르지 못한다(검토 R6).
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

    /// 같은 저장소에서 관문이 `target`에 병합한 커밋(브랜치마다 마지막 것). 순서는 git의 첫 부모 줄이 정한다:
    /// 초 단위 시각은 같은 초의 두 병합을 가르지 못한다(검토 Codex R4).
    pub fn gate_merges(&self, repo: RepoId, target: &str) -> rusqlite::Result<Vec<String>> {
        self.lock()
            .prepare(
                "SELECT merged FROM gates WHERE repo = ?1 AND target = ?2 AND merged IS NOT NULL",
            )?
            .query_map(params![repo.0, target], |r| r.get(0))?
            .collect()
    }

    /// audit 가운데 DB로 아는 것: 이번 차례의 흔적 구간이 빈틈없는지, 담당과 그 자손(같은 저장소)이 관문 동사 밖으로 관문 낱말을 보냈는지.
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
            // `human-input`·`setup-failed` 본문은 사람과 설정 스크립트의 글이다. 에이전트에게 반려를 돌리지 않는다(관문 검토 P2-1).
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

    /// `loom fail`: 관문이 큐에 들기 전의 브랜치를 멈추고 알린다.
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

    /// 거절한 관문 호출을 남긴다. 사람과 audit가 이벤트 기록에서 본다.
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

    /// 묶음에 들 브랜치가 있는 (저장소, 대상). 가장 오래 기다린 대상부터다.
    pub fn queued_targets(&self) -> rusqlite::Result<Vec<(RepoId, String)>> {
        self.lock()
            .prepare(&format!(
                "SELECT repo, target FROM gates WHERE {} GROUP BY repo, target ORDER BY min(seq)",
                settled()
            ))?
            .query_map([], |r| Ok((RepoId(r.get(0)?), r.get(1)?)))?
            .collect()
    }

    /// 한 대상에서 묶음에 들 브랜치. pass 판정 순서다.
    pub fn queue(&self, repo: RepoId, target: &str) -> rusqlite::Result<Vec<Queued>> {
        self.lock()
            .prepare(&format!(
                "{QUEUED} WHERE repo = ?1 AND target = ?2 AND {} ORDER BY seq",
                settled()
            ))?
            .query_map(params![repo.0, target], read_queued)?
            .collect()
    }

    /// 묶음을 연다: `batches` 줄을 만들고 그 브랜치들을 `Merging`으로 바꾼다. 한 트랜잭션이다.
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

    /// 검사를 지난 묶음 끝과 쌓은 대상 커밋. 데몬이 게시 중에 끝나면 다시 띄운 데몬이 이 값으로 게시를 잇는다.
    pub fn batch_checked(&self, id: BatchId, base: &str, tip: &str) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE batches SET base = ?2, tip = ?3 WHERE id = ?1",
            params![id.0, base, tip],
        )?;
        Ok(())
    }

    /// 묶음을 닫는다. 아직 `Merging`인 브랜치가 있으면 줄을 남긴다: 끝 상태를 쓰지 못한 브랜치를 큐가 다시 거둔다(검토 R3).
    pub fn batch_close(&self, id: BatchId) -> rusqlite::Result<()> {
        self.lock().execute(
            "DELETE FROM batches WHERE id = ?1
             AND NOT EXISTS (SELECT 1 FROM gates WHERE batch = ?1 AND state = '\"merging\"')",
            [id.0],
        )?;
        Ok(())
    }

    /// 데몬이 끝내지 못한 묶음. 데몬이 시작할 때 거둔다.
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

    /// 묶음 없이 `Merging`인 브랜치. 데몬은 이 꼴을 만들지 않는다(`batch_open`이 함께 쓴다): 마이그레이션 36이 옛 규칙의 차례를 이렇게 둔다.
    pub fn orphans(&self) -> rusqlite::Result<Vec<(RepoId, String)>> {
        self.lock()
            .prepare("SELECT repo, branch FROM gates WHERE state = '\"merging\"' AND batch IS NULL ORDER BY rowid")?
            .query_map([], |r| Ok((RepoId(r.get(0)?), r.get(1)?)))?
            .collect()
    }

    /// 묶음의 브랜치를 큐에 다시 세운다. 큐 자리(`seq`)는 그대로다: 앞에 섰던 브랜치가 다음 묶음에서도 앞이다.
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

    /// 병합 큐가 브랜치 하나를 끝낸다: 병합, 반려, 실패. 받는 노드에 loom이 알린다.
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

    /// 관문 호출 하나: 부를 수 있는 노드인지 보고, 전이하고, `write`로 줄을 쓰고, 받는 노드에 메시지를 넣는다. 모두 한 트랜잭션이다.
    /// `write`는 새 상태를 담은 줄을 받는다. 줄이 없던 브랜치는 `Working`에서 시작한다(`loom ready`만 줄을 만든다).
    /// 차례가 판정·병합 전에 `Working`·`Failed`로 돌아가면 그 차례의 검토자에게 `withdrawn`을 보낸다(`withdraw`).
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
            // `loom ready`의 `write`가 줄 전체를 쓴다. 이 값은 받는 노드를 고를 때만 쓴다.
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
            for receiver in protocol::receivers(call, row.target_kind()) {
                let to_node = match receiver {
                    Receiver::Gate => row.gate,
                    Receiver::Owner => row.owner,
                    Receiver::Root if exists(&tx, row.owner)? => root_of(&tx, row.owner)?,
                    // 담당을 지웠으면 루트를 모른다. 관문 줄과 흔적은 남는다.
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

/// 부른 쪽이 이 동사를 부를 수 있는가(`protocol::sender`).
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

/// 차례를 거두는 전이: 검토나 병합을 기다리던 차례가 담당에게 돌아가거나 멈췄다.
fn withdrawn(from: GateState, to: GateState) -> bool {
    matches!(
        (from, to),
        (
            GateState::Ready | GateState::Passed | GateState::Merging,
            GateState::Working | GateState::Failed
        )
    )
}

/// 거둔 차례의 검토자에게 `withdrawn`을 보낸다. 검토자는 관문의 산 자식 가운데 담당과 부른 노드가 아니고,
/// 관문의 작업 폴더를 함께 쓰고(`--worktree` 없이 띄웠다), 이번 `loom ready` 뒤에 떴거나(`mark`보다 큰 번호) 이 브랜치의 마지막 ready 흔적 뒤에 판정한 노드다.
/// 차례는 흔적 번호로 가른다: 초 단위 시각은 같은 초의 앞 차례 판정을 가르지 못한다(시험 실측).
/// 한 관문 아래 두 차례가 겹치면 다른 브랜치의 검토자도 받는다. 본문이 그때는 무시하라고 알린다.
/// 메시지 연동이 있는 에이전트는 조정으로 받아 지금 턴에 멈춘다.
fn withdraw(
    tx: &Transaction,
    caller: Caller,
    row: &Row,
    branch: &str,
    trace: &GateTrace,
) -> rusqlite::Result<Vec<NodeId>> {
    let reviewers: Vec<(NodeId, Option<AgentId>)> = tx
        .prepare(&format!(
            "SELECT n.id, n.agent FROM nodes n WHERE {} AND n.id != ?3 AND (n.id > ?4 OR {}) ORDER BY n.id",
            reviewer("?1", "?2"),
            voted("?5", "?2")
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

/// `n`이 관문 `gate`의 산 검토자다: 담당이 아니고, 관문의 작업 폴더를 함께 쓴다(`--worktree` 없이 띄웠다). 인자는 SQL 식이다.
fn reviewer(gate: &str, owner: &str) -> String {
    format!(
        "n.parent = {gate} AND n.id != {owner} AND n.process_exited = 0 AND n.task IS (SELECT task FROM nodes WHERE id = {gate})"
    )
}

/// `n`이 이 브랜치의 이번 차례에 판정했다: 담당의 마지막 ready 흔적 뒤에 판정 흔적이 있다.
/// ready는 담당의 것만 본다: 다른 저장소의 같은 이름 브랜치가 차례를 가르지 않게 한다(검토 N1). 판정은 관문의 자식인 `n`의 것이라 같은 저장소다.
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

/// 관문 메시지 하나를 넣는다. 에이전트 호출이면 그 노드가, 병합 큐면 loom이 보낸다.
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

/// 부모가 코디면 그 부모. 코디 밑 워커의 관문은 코디다.
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

/// 묶음에 들 수 있는 관문 줄: pass를 받았고, 이번 `loom ready` 뒤에 뜬 산 검토자가 모두 판정했다.
/// 첫 pass에 바로 묶으면 둘째 검토자의 block이 `Merging`에서 거절되고 브랜치가 병합된다(검토 B1, 관문 결정 #4041 (a)).
/// 어느 브랜치든 판정했으면 끝난 것으로 센다: 겹친 다른 차례의 검토자는 이 브랜치를 판정하지 않으므로 끝날 때까지 이 브랜치를 막는다.
/// 그 노드는 ready 뒤에 떴으므로 그 판정은 모두 이번 차례 뒤다.
/// 판정 없이 끝난 검토자는 세지 않는다. 그때는 깨우기가 없어 큐의 한 번씩 보기(`TICK`)가 줍는다.
fn settled() -> String {
    format!(
        "state = '\"passed\"' AND NOT EXISTS (SELECT 1 FROM nodes n WHERE {} AND n.id > gates.mark
            AND NOT EXISTS (SELECT 1 FROM events e WHERE e.entry ->> '$.kind' = 'message'
                AND e.entry ->> '$.gate.verb' = 'verdict' AND e.entry ->> '$.from.id' = n.id))",
        reviewer("gates.gate", "gates.owner")
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

/// 저장소의 그 브랜치 관문 줄.
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

/// 부모를 따라 올라간 맨 위 노드.
fn root_of(db: &Connection, node: NodeId) -> rusqlite::Result<NodeId> {
    let mut at = node;
    while let Some(parent) = parent_of(db, at)? {
        at = parent;
    }
    Ok(at)
}

/// `top`과 그 아래에서 `top`과 같은 저장소의 노드. 다른 저장소의 자손은 이 브랜치와 관계없다(검토 P2-5).
fn tree_of(db: &Connection, top: NodeId) -> rusqlite::Result<Vec<NodeId>> {
    db.prepare(
        "WITH RECURSIVE tree(id) AS (SELECT ?1 UNION SELECT n.id FROM nodes n JOIN tree t ON n.parent = t.id)
         SELECT t.id FROM tree t JOIN nodes n ON n.id = t.id WHERE n.repo = (SELECT repo FROM nodes WHERE id = ?1)",
    )?
    .query_map([top.0], |row| Ok(NodeId(row.get(0)?)))?
    .collect()
}
