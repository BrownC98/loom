//! 병합 관문 상태: 저장소·브랜치마다 한 줄(`gates`). 전이는 `protocol::step`만 정한다. 이 모듈은 줄을 읽고 쓰고,
//! 부를 수 있는 노드를 보고, 받는 노드에 고정 꼴 메시지를 넣고, 같은 트랜잭션에서 흔적을 남긴다.
//! git으로 아는 사실(담당 워크트리의 브랜치, 미커밋 변경, 포함 관계, 커밋)은 데몬이 읽어 넘긴다.
//! 근거: 출력 표준 R8 v5 ④·⑤(2026-10-23 사람 결정 Q559 A).

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use super::events::{self, Entry, GateTrace, Via, Who};
use super::mailbox::{self, Stamp};
use super::{Graph, Kind, NodeId, Refusal, from_json, name_of, parent_of, place_of, to_json};
use crate::protocol::{
    self, CI_ROLE, COORDINATOR_ROLE, Finding, GateCall, GateFacts, GateState, GateVerb, Judgment,
    MergeEnd, MessageKind, Receiver, Sender,
};

/// `loom ready`이 데몬에서 읽은 브랜치 사실.
pub struct Round<'a> {
    pub branch: &'a str,
    pub target: &'a str,
    /// 담당 워크트리(데몬이 보는 경로).
    pub worktree: &'a str,
    pub head: &'a str,
}

/// 브랜치 담당의 관문.
#[derive(Debug, PartialEq)]
pub enum GateChoice {
    /// 부모가 코디다. 대상은 코디 워크트리의 브랜치다.
    Coordinator { gate: NodeId, worktree: String },
    /// 같은 저장소의 살아 있는 CI 담당자. 대상은 원래 체크아웃의 브랜치다.
    Ci { gate: NodeId },
}

/// `loom merge`가 할 일. 관문 줄의 이번 차례.
#[derive(Debug, PartialEq)]
pub struct MergeTask {
    pub owner: NodeId,
    pub worktree: String,
    pub target: String,
    /// 검토한 커밋. 검사와 병합은 이 커밋만 쓴다.
    pub head: String,
    /// audit 시작 시각: 이번 차례의 `loom ready`.
    pub since: i64,
    /// 이번 `loom merge` 실행 번호. 끝 보고가 같은 번호를 가져온다.
    pub run: i64,
    /// 이 실행을 시작하기 전 상태. `Merging`이면 앞 실행의 끝 보고가 오지 않았다.
    pub from: GateState,
    /// 이 브랜치를 마지막으로 병합한 커밋.
    pub merged: Option<String>,
}

/// `loom merge`가 끝난 꼴과 알릴 값.
pub struct Ended<'a> {
    pub end: MergeEnd,
    /// 병합 커밋. 병합했을 때만 있다.
    pub merged: Option<&'a str>,
    pub cause: Option<String>,
    /// `loom merge` 로그 파일.
    pub log: Option<String>,
    /// 받는 노드에 덧붙일 본문 줄(배포 결과, 되찾은 병합).
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
    since: i64,
    run: i64,
    merged: Option<String>,
}

/// 관문 호출 하나가 받는 노드에 보내는 칸(`protocol::GateFacts`의 호출 쪽 값)과 본문 줄, 흔적에 쓸 audit 결과.
/// 브랜치·커밋·워크트리·대상·검토 모델은 관문 줄과 부른 노드에서 `apply`가 채운다.
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
    /// 담당 `owner`의 관문. 부모가 코디면 코디, 아니면 같은 저장소에서 프로세스가 사는 CI 담당자 하나다.
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
                "SELECT id FROM nodes WHERE role = ?1 AND repo = ?2 AND process_exited = 0 AND id != ?3 ORDER BY id",
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

    /// `loom ready`: 담당이 브랜치를 관문에 보낸다. 데몬이 담당·미커밋·포함 관계를 이미 봤다. 돌려준 값은 관문 이름이다.
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

    /// `loom verdict`: 관문이나 그 검토자가 판정한다. block은 결함 파일이 있어야 한다.
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

    /// `loom merge` 시작: 관문만, 판정 pass 뒤에만 부른다. 새 실행 번호를 받는다. 앞 실행의 끝 보고는 그 뒤 받지 않는다.
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

    /// 이 브랜치의 지금 차례. 병합 끝 보고를 git에서 확인할 때 쓴다.
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

    /// 브랜치 담당의 작업 폴더 이름과 담당의 워크트리. 브랜치 이름이 아니라 관문 줄의 담당이 정한다:
    /// 브랜치 이름을 바꿔도, 같은 이름을 다시 써도 그 차례의 작업이다. 관문 줄이 없거나 담당 노드가 지워졌거나 폴더가 없으면 없다.
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

    /// 같은 저장소에서 관문이 `target`에 병합한 커밋(브랜치마다 마지막 것). 순서는 git의 첫 부모 줄이 정한다:
    /// 초 단위 시각은 같은 초의 두 병합을 가르지 못한다(검토 Codex R4).
    pub fn gate_merges(&self, caller: NodeId, target: &str) -> rusqlite::Result<Vec<String>> {
        let db = self.lock();
        let repo = place_of(&db, caller)?.repo;
        db.prepare(
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

    /// `loom merge` 끝. `run`이 지금 실행 번호가 아니면 받지 않는다.
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

    /// `loom fail`: 관문이 브랜치를 멈추고 루트에 알린다.
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

    /// 관문 호출 하나: 부를 수 있는 노드인지 보고, 전이하고, `write`로 줄을 쓰고, 받는 노드에 메시지를 넣는다. 모두 한 트랜잭션이다.
    /// `write`는 새 상태를 담은 줄을 받는다. 줄이 없던 브랜치는 `Working`에서 시작한다(`loom ready`만 줄을 만든다).
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
            // `loom ready`의 `write`가 줄 전체를 쓴다. 이 값은 받는 노드를 고를 때만 쓴다.
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
                // 담당을 지웠으면 루트를 모른다. 관문 줄과 흔적은 남는다.
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

/// 부르는 노드의 저장소에서 그 브랜치의 관문 줄.
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
