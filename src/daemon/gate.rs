//! 관문 경로(`/api/agent/{ready,verdict,merge,merged,fail}`): git으로 브랜치 사실을 읽어 그래프의 관문 상태에 넘긴다.
//! 무거운 일(검사 스크립트, 병합, push, 배포 확인)은 `loom merge`를 부른 CLI 프로세스가 워크트리 옆에서 한다. 데몬은 Windows에 있을 수 있다.
//! 모든 호출과 거절은 흔적을 남긴다. 근거: 출력 표준 R8 v5 ③·④·⑤(2026-10-23 사람 결정 Q559 A).

use std::path::Path;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

use super::{AgentError, App, for_agent};
use crate::git;
use crate::graph::{Ended, GateChoice, MergeTask, NodeId, Refusal, Round, Via};
use crate::protocol::{
    self, CHECK_SCRIPT, Finding, GateState, GateVerb, Judgment, MergeEnd, SHIP_SCRIPT,
};
use crate::worktree;

/// CLI가 요청마다 보내는 입구 헤더: 부른 `loom` 명령 이름과 판.
pub const TOOL_HEADER: &str = "loom-tool";
pub const VERSION_HEADER: &str = "loom-version";

#[derive(Serialize, Deserialize)]
pub struct BranchRequest {
    pub branch: String,
}

#[derive(Serialize, Deserialize)]
pub struct ReadyRequest {
    pub branch: String,
    /// 부른 프로세스의 작업 폴더(부른 쪽이 읽는 글 그대로). 노드가 등록한 워크트리가 아닌 곳에서 일할 때 그 폴더가 담당 증거다.
    pub cwd: String,
}

#[derive(Serialize, Deserialize)]
pub struct Readied {
    pub gate: String,
    pub target: String,
}

#[derive(Serialize, Deserialize)]
pub struct VerdictRequest {
    pub branch: String,
    pub judgment: Judgment,
    /// 결함 파일 경로(부른 쪽이 읽는 글 그대로). block이면 꼭 있다.
    pub findings: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct FailRequest {
    pub branch: String,
    pub cause: String,
}

/// `loom merge`가 데몬에서 받는 할 일. 경로는 데몬이 보는 글이다. CLI가 자기 쪽에서 띄운다(`wsl::command`).
#[derive(Serialize, Deserialize)]
pub enum MergePlan {
    /// audit가 찾은 일이 있어 병합하지 않았다. 관문 상태는 이미 바뀌었다.
    Stopped {
        end: MergeEnd,
        findings: Vec<String>,
    },
    /// 앞 실행이 이미 병합했고 그 끝 보고만 잃었다. 데몬이 git에서 병합 커밋을 찾아 `Merged`로 남겼다.
    Recorded { sha: String },
    Go {
        /// 담당 브랜치의 워크트리. 검사 스크립트를 여기서 돌린다.
        worktree: String,
        /// 검토한 커밋. 워크트리가 이 커밋에 있어야 검사하고, 이 커밋을 병합한다.
        head: String,
        /// 실행 번호. 끝 보고(`MergedRequest`)에 그대로 넣는다.
        run: i64,
        into: MergeInto,
    },
}

#[derive(Serialize, Deserialize)]
pub enum MergeInto {
    /// 원래 체크아웃의 브랜치(main): 그 체크아웃에서 `ship.sh`가 병합·push·배포 확인을 한다.
    Main { checkout: String },
    /// 코디 브랜치: 그 워크트리에서 검토한 커밋을 `git merge --no-ff`.
    Branch { worktree: String },
}

#[derive(Serialize, Deserialize)]
pub struct MergedRequest {
    pub branch: String,
    pub run: i64,
    pub result: MergeResult,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum MergeResult {
    Merged {
        sha: String,
        deploy: Deploy,
        log: String,
    },
    /// 담당이 고칠 일(검사 실패, 충돌, 대상이 움직임).
    Rework {
        cause: String,
        log: String,
    },
    Failed {
        cause: String,
        log: String,
    },
    /// 관문이 다시 부르면 될 일. `Merging`에 남는다.
    Stopped {
        cause: String,
        log: String,
    },
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq)]
pub enum Deploy {
    Confirmed,
    Failed,
    /// 코디 브랜치로의 병합은 배포하지 않는다.
    NotApplicable,
}

/// 요청이 들어온 입구. 헤더가 없으면(화면, 옛 CLI, 손으로 부른 API) 경로다.
pub fn via(headers: &HeaderMap, route: &str) -> Via {
    let header = |name| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    Via {
        tool: header(TOOL_HEADER).unwrap_or_else(|| route.to_owned()),
        version: header(VERSION_HEADER),
    }
}

impl App {
    /// 손으로 쓴 글(`note`·`answer`·`done`)이 종류 낱말로 시작하면 거절한다. 판정·병합 낱말은 관문 스크립트가 있는 저장소에서만 본다:
    /// 없는 저장소(ktasearch)는 `loom verdict`·`loom merge`가 돌지 않아 옛 글을 쓴다(이전 기간).
    pub(super) fn kind_word(&self, node: NodeId, body: &str) -> Result<(), Refusal> {
        let Some(found) = protocol::kind_word(body) else {
            return Ok(());
        };
        if found.gated_only && !self.gated_repo(node)? {
            return Ok(());
        }
        Err(Refusal::KindWord {
            word: found.word.to_owned(),
            verb: found.verb.to_owned(),
        })
    }

    /// `loom done`의 담당 브랜치 경고. 코디 밑이 아닌 노드의 워크트리 브랜치가 대상(원래 체크아웃의 브랜치)에 병합되지 않았고
    /// 그 커밋으로 `loom ready`도 보내지 않았으면 경고 글이다. 거절하지 않는다: 완료는 중간 결과도 나른다.
    /// 근거: 요청 #3067(ktasearch: 워커가 브랜치를 알리지 않고 끝내 1.5시간 아무도 보지 않았다).
    pub(super) async fn unreadied(&self, node: NodeId) -> Result<Option<String>, Refusal> {
        if self.graph.under_coordinator(node)? {
            return Ok(None);
        }
        let place = self.graph.place_of(node)?;
        let repo = self.repo_of(node)?;
        branch_warning(&repo, &place.worktree, |branch, head| {
            self.graph.ready_sent(node, branch, head)
        })
        .await
    }

    /// 노드의 저장소가 관문 계약(원래 체크아웃의 `scripts/gate/check.sh`, `ship.sh`)을 가졌는가.
    pub(super) fn gated_repo(&self, node: NodeId) -> Result<bool, Refusal> {
        let repo = self.repo_of(node)?;
        Ok(repo.join(CHECK_SCRIPT).is_file() && repo.join(SHIP_SCRIPT).is_file())
    }

    /// 관문 호출의 결과를 돌려준다. 거절은 흔적으로 남긴다. DB 오류는 요청의 잘못이 아니므로 남기지 않는다.
    fn traced<T>(
        &self,
        node: NodeId,
        verb: GateVerb,
        branch: &str,
        via: &Via,
        result: Result<T, Refusal>,
    ) -> Result<Json<T>, AgentError> {
        match result {
            Ok(value) => {
                self.turn_ends.called(node);
                Ok(Json(value))
            }
            Err(reason) => {
                if !matches!(reason, Refusal::Db(_)) {
                    self.graph
                        .record_gate_refusal(node, verb, branch, &reason, Some(via))
                        .map_err(|e| for_agent(Refusal::from(e)))?;
                }
                Err(for_agent(reason))
            }
        }
    }

    fn repo_of(&self, node: NodeId) -> Result<std::path::PathBuf, Refusal> {
        let place = self.graph.place_of(node)?;
        self.repos
            .path(place.repo)
            .map_err(|e| Refusal::Git(e.to_string()))
    }

    async fn ready(
        &self,
        node: NodeId,
        branch: &str,
        cwd: &str,
        via: &Via,
    ) -> Result<Readied, Refusal> {
        let place = self.graph.place_of(node)?;
        let repo = self.repo_of(node)?;
        let mine = owned_worktree(&repo, &place.worktree, branch, cwd).await?;
        if !worktree::changes(&mine)
            .await
            .map_err(git_problem)?
            .is_empty()
        {
            return Err(Refusal::Uncommitted(branch.to_owned()));
        }
        let base = base_branch(&repo).await?;
        let (gate, target) = match self.graph.gate_for(node)? {
            GateChoice::Coordinator { gate, worktree } => {
                let theirs = worktree::find(&repo, &worktree)
                    .await
                    .map_err(git_problem)?;
                match theirs.branch {
                    Some(target) if target != base => (gate, target),
                    // 코디가 원래 체크아웃(main)에서 일한다: 워커 브랜치는 코디가 받는다. 워커는 `loom done`으로 알린다.
                    _ => return Err(Refusal::SameAsTarget(base)),
                }
            }
            GateChoice::Ci { gate } => (gate, base),
        };
        if target == branch {
            return Err(Refusal::SameAsTarget(branch.to_owned()));
        }
        contains(&repo, &target, branch).await?;
        if contains(&repo, branch, &target).await.is_ok() {
            return Err(Refusal::NothingToMerge {
                branch: branch.to_owned(),
                target,
            });
        }
        // 관문 에이전트는 WSL에서 이 경로를 읽는다. Windows 데몬이 보는 UNC 글자 대신 배포판 안의 경로를 쓴다.
        let worktree =
            crate::wsl::location(&mine.path).map_or_else(|| mine.path.clone(), |w| w.path);
        let round = Round {
            branch,
            target: &target,
            worktree: &worktree,
            head: &mine.head,
        };
        let gate = match self.gated_repo(node)? {
            true => self.graph.ready(node, &round, gate, Some(via))?,
            false => self.graph.announce_ready(node, &round, gate, Some(via))?,
        };
        Ok(Readied { gate, target })
    }

    /// `loom merge` 앞 단계: 상태를 `Merging`으로 바꾸고 audit를 돌린다. 찾은 일이 있으면 병합하지 않고 끝낸다.
    /// `Merging`에서 다시 불렀고 검토한 커밋이 이미 대상에 있으면 앞 실행이 병합한 뒤 보고 전에 끝났다(검토 P1-2):
    /// 원래 체크아웃이면 같은 일을 다시 준다. ship.sh가 그 병합을 이어 push·배포 확인을 하고 보고한다(코디 결정 7).
    /// 코디 브랜치는 push가 없으므로 git에서 그 병합 커밋을 찾아 바로 남긴다.
    async fn merge(&self, node: NodeId, branch: &str, via: &Via) -> Result<MergePlan, Refusal> {
        let repo = self.repo_of(node)?;
        if !self.gated_repo(node)? {
            return Err(Refusal::NoGateScripts);
        }
        let task = self.graph.merge_start(node, branch, Some(via))?;
        let into = self.merge_into(&repo, &task.target).await?;
        let go = |task: MergeTask, into| MergePlan::Go {
            worktree: task.worktree,
            head: task.head,
            run: task.run,
            into,
        };
        if task.from == GateState::Merging
            && let Ok(()) = contains(&repo, &task.head, &task.target).await
        {
            if let MergeInto::Main { .. } = into {
                return Ok(go(task, into));
            }
            let Some(sha) = merge_commit(&repo, &task.head, &task.target).await? else {
                let cause = format!(
                    "the reviewed commit {} is in {}, but no merge commit on {} has it as a parent",
                    task.head, task.target, task.target
                );
                return self.stop(node, branch, &task, Vec::new(), Some(cause), via);
            };
            let lines = vec![
                "recovered: an earlier run merged this commit, and its report did not arrive"
                    .to_owned(),
            ];
            self.record_merged(
                node,
                branch,
                &task,
                &sha,
                MergedNotes { lines, log: None },
                via,
            )
            .await?;
            return Ok(MergePlan::Recorded { sha });
        }
        let mut findings = self.graph.audit(task.owner, task.since)?;
        let now = git::run(&repo, &["rev-parse", &format!("refs/heads/{branch}")])
            .await
            .map_err(Refusal::Git)?;
        if now.trim() != task.head {
            findings.push(Finding::HeadMoved {
                reviewed: task.head.clone(),
                now: now.trim().to_owned(),
            });
        }
        match contains(&repo, &task.target, &task.head).await {
            Ok(()) => {}
            Err(Refusal::NotAncestor { target, .. }) => {
                findings.push(Finding::TargetMoved { target })
            }
            Err(e) => return Err(e),
        }
        if protocol::audit_end(&findings).is_some() {
            return self.stop(node, branch, &task, findings, None, via);
        }
        Ok(go(task, into))
    }

    /// 병합하지 않고 끝낸다. audit 결과가 끝을 정하고, 결과 없이 `cause`만 있으면 담당이 고칠 일이다.
    fn stop(
        &self,
        node: NodeId,
        branch: &str,
        task: &MergeTask,
        findings: Vec<Finding>,
        cause: Option<String>,
        via: &Via,
    ) -> Result<MergePlan, Refusal> {
        let end = protocol::audit_end(&findings).unwrap_or(MergeEnd::Rework);
        let shown = findings
            .iter()
            .map(Finding::to_string)
            .chain(cause.iter().cloned())
            .collect();
        let ended = Ended {
            end,
            merged: None,
            cause,
            log: None,
            lines: Vec::new(),
            findings,
        };
        self.graph
            .merge_end(node, branch, task.run, ended, Some(via))?;
        Ok(MergePlan::Stopped {
            end,
            findings: shown,
        })
    }

    /// 대상이 원래 체크아웃의 브랜치면 그 체크아웃, 아니면 대상 브랜치의 워크트리(코디).
    async fn merge_into(&self, repo: &Path, target: &str) -> Result<MergeInto, Refusal> {
        if target == base_branch(repo).await? {
            return Ok(MergeInto::Main {
                checkout: repo.to_string_lossy().into_owned(),
            });
        }
        let found = worktree::worktrees(repo)
            .await
            .into_iter()
            .find(|w| w.branch.as_deref() == Some(target))
            .ok_or_else(|| Refusal::Git(format!("no worktree has {target}")))?;
        Ok(MergeInto::Branch {
            worktree: found.path,
        })
    }

    /// 병합 커밋 `sha`를 git에서 확인하고 `Merged`로 남긴다. 대상 브랜치가 원래 체크아웃의 브랜치면
    /// 앞선 관문 병합과 이 병합 사이의 대상 커밋을 관문 밖 커밋으로 붙인다(막지 않는다).
    /// 확인만 한다: 관문이 손으로 병합하고 그 커밋을 보고해도 받는다(남은 위험, docs/design.md 「병합 관문」).
    async fn record_merged(
        &self,
        node: NodeId,
        branch: &str,
        task: &MergeTask,
        sha: &str,
        notes: MergedNotes,
        via: &Via,
    ) -> Result<GateState, Refusal> {
        let repo = self.repo_of(node)?;
        let not_merged = || Refusal::NotMerged {
            sha: sha.to_owned(),
            target: task.target.clone(),
            head: task.head.clone(),
        };
        if sha.is_empty() || sha == task.head {
            return Err(not_merged());
        }
        for (older, newer) in [(task.head.as_str(), sha), (sha, task.target.as_str())] {
            match contains(&repo, older, newer).await {
                Ok(()) => {}
                Err(Refusal::NotAncestor { .. } | Refusal::Git(_)) => return Err(not_merged()),
                Err(e) => return Err(e),
            }
        }
        let mut findings = Vec::new();
        if task.target == base_branch(&repo).await? {
            let known = self.graph.gate_merges(node, &task.target)?;
            let listed = git::run(&repo, &["rev-list", "--first-parent", &format!("{sha}^1")])
                .await
                .map_err(Refusal::Git)?;
            findings = protocol::out_of_gate(&listed, &known);
        }
        let ended = Ended {
            end: MergeEnd::Merged,
            merged: Some(sha),
            cause: None,
            log: notes.log,
            lines: notes.lines,
            findings,
        };
        self.graph
            .merge_end(node, branch, task.run, ended, Some(via))
    }

    /// `loom merge`를 부른 CLI의 끝 보고. 병합은 어느 실행의 보고든 git에서 확인되면 받는다(코디 결정 3).
    /// 이미 남긴 같은 병합을 다시 보고하면(다시 붙은 보고) 그대로 받는다. 병합 아닌 끝은 지금 실행의 것만 받는다.
    async fn merged(
        &self,
        node: NodeId,
        req: &MergedRequest,
        via: &Via,
    ) -> Result<GateState, Refusal> {
        let (end, cause, log) = match &req.result {
            MergeResult::Merged { sha, deploy, log } => {
                let task = self.graph.merge_task(node, &req.branch)?;
                if task.from == GateState::Merged && task.merged.as_deref() == Some(sha.as_str()) {
                    return Ok(GateState::Merged);
                }
                let notes = MergedNotes {
                    lines: vec![format!("deploy: {deploy:?}")],
                    log: Some(log.clone()),
                };
                return self
                    .record_merged(node, &req.branch, &task, sha, notes, via)
                    .await;
            }
            MergeResult::Rework { cause, log } => (MergeEnd::Rework, cause, log),
            MergeResult::Failed { cause, log } => (MergeEnd::Failed, cause, log),
            MergeResult::Stopped { cause, log } => (MergeEnd::Stopped, cause, log),
        };
        let ended = Ended {
            end,
            merged: None,
            cause: Some(cause.clone()),
            log: Some(log.clone()),
            lines: Vec::new(),
            findings: Vec::new(),
        };
        self.graph
            .merge_end(node, &req.branch, req.run, ended, Some(via))
    }
}

/// 대상 첫 부모 줄에서 `head`를 둘째 이후 부모로 가진 병합 커밋. ship.sh·코디 병합은 늘 `--no-ff`다.
async fn merge_commit(repo: &Path, head: &str, target: &str) -> Result<Option<String>, Refusal> {
    let listed = git::run(repo, &["log", "--first-parent", "--format=%H %P", target])
        .await
        .map_err(Refusal::Git)?;
    Ok(listed.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let sha = parts.next()?;
        parts.skip(1).any(|p| p == head).then(|| sha.to_owned())
    }))
}

/// 원래 체크아웃이 가리키는 브랜치. CI 관문의 대상이다(보통 main).
async fn base_branch(repo: &Path) -> Result<String, Refusal> {
    worktree::worktrees(repo)
        .await
        .into_iter()
        .find(|w| w.original)
        .and_then(|w| w.branch)
        .ok_or_else(|| Refusal::Git("the main checkout is not on a branch".into()))
}

/// `target`이 `branch`에 포함되는가. git은 포함되지 않으면 글 없이 1로 끝난다.
async fn contains(repo: &Path, target: &str, branch: &str) -> Result<(), Refusal> {
    match git::run(repo, &["merge-base", "--is-ancestor", target, branch]).await {
        Ok(_) => Ok(()),
        Err(e) if e.is_empty() => Err(Refusal::NotAncestor {
            target: target.to_owned(),
            branch: branch.to_owned(),
        }),
        Err(e) => Err(Refusal::Git(e)),
    }
}

/// `branch`를 담당하는 워크트리. 노드가 등록한 워크트리가 그 브랜치면 그것이고,
/// 아니면 부른 프로세스의 작업 폴더가 git으로 그 브랜치 워크트리일 때(원래 체크아웃의 노드가 다른 워크트리에서 고칠 때) 그 워크트리다.
async fn owned_worktree(
    repo: &Path,
    registered: &str,
    branch: &str,
    cwd: &str,
) -> Result<worktree::Worktree, Refusal> {
    let mine = worktree::find(repo, registered)
        .await
        .map_err(git_problem)?;
    if mine.branch.as_deref() == Some(branch) {
        return Ok(mine);
    }
    // 폴더 접두사는 안쪽 워크트리·중첩 저장소를 가르지 못한다. git이 푼 최상위 폴더만 믿는다.
    let cwd = Path::new(cwd);
    let top = git::run(cwd, &["rev-parse", "--show-toplevel"])
        .await
        .map_err(Refusal::Git)?;
    let top = git::from_git_path(cwd, top.trim());
    worktree::worktrees(repo)
        .await
        .into_iter()
        .find(|w| w.branch.as_deref() == Some(branch) && w.path == top)
        .ok_or(Refusal::NotOwner {
            branch: branch.to_owned(),
            yours: mine.branch,
        })
}

/// 병합 보고에 덧붙일 본문 줄(배포 결과, 되찾은 병합)과 `loom merge` 로그.
struct MergedNotes {
    lines: Vec<String>,
    log: Option<String>,
}

/// `worktree`의 브랜치가 원래 체크아웃의 브랜치가 아니고, 거기에 병합되지 않았고, 그 커밋으로 `ready`를 보내지 않았으면(`ready_sent`) 경고 글.
async fn branch_warning(
    repo: &Path,
    worktree: &str,
    ready_sent: impl FnOnce(&str, &str) -> rusqlite::Result<bool>,
) -> Result<Option<String>, Refusal> {
    let mine = worktree::find(repo, worktree).await.map_err(git_problem)?;
    let base = base_branch(repo).await?;
    let Some(branch) = mine.branch.filter(|b| *b != base) else {
        return Ok(None);
    };
    if contains(repo, &mine.head, &base).await.is_ok() || ready_sent(&branch, &mine.head)? {
        return Ok(None);
    }
    Ok(Some(format!(
        "branch {branch} ({}) is not merged into {base}, and no `loom ready {branch}` was sent for this commit",
        short(&mine.head)
    )))
}

/// 경고 글의 짧은 커밋.
fn short(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

fn git_problem(e: worktree::RepoProblem) -> Refusal {
    Refusal::Git(e.to_string())
}

pub(super) async fn agent_ready(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<ReadyRequest>,
) -> Result<Json<Readied>, AgentError> {
    let node = app.caller(&headers)?;
    let via = via(&headers, "/api/agent/ready");
    let result = app.ready(node, &req.branch, &req.cwd, &via).await;
    app.traced(node, GateVerb::Ready, &req.branch, &via, result)
}

pub(super) async fn agent_verdict(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<VerdictRequest>,
) -> Result<Json<GateState>, AgentError> {
    let node = app.caller(&headers)?;
    let via = via(&headers, "/api/agent/verdict");
    let result = app.graph.verdict(
        node,
        &req.branch,
        req.judgment,
        req.findings.as_deref(),
        Some(&via),
    );
    app.traced(node, GateVerb::Verdict, &req.branch, &via, result)
}

pub(super) async fn agent_merge(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<BranchRequest>,
) -> Result<Json<MergePlan>, AgentError> {
    let node = app.caller(&headers)?;
    let via = via(&headers, "/api/agent/merge");
    let result = app.merge(node, &req.branch, &via).await;
    app.traced(node, GateVerb::Merge, &req.branch, &via, result)
}

pub(super) async fn agent_merged(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<MergedRequest>,
) -> Result<Json<GateState>, AgentError> {
    let node = app.caller(&headers)?;
    let via = via(&headers, "/api/agent/merged");
    let result = app.merged(node, &req, &via).await;
    app.traced(node, GateVerb::Merge, &req.branch, &via, result)
}

pub(super) async fn agent_fail(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<FailRequest>,
) -> Result<Json<GateState>, AgentError> {
    let node = app.caller(&headers)?;
    let via = via(&headers, "/api/agent/fail");
    let result = app.graph.fail(node, &req.branch, &req.cause, Some(&via));
    app.traced(node, GateVerb::Fail, &req.branch, &via, result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::test_run as run;

    /// 요청 #3067: 담당 브랜치가 병합되지 않았고 그 커밋으로 `loom ready`도 없으면 `loom done`에 경고가 붙는다(거절하지 않는다).
    #[tokio::test]
    async fn done_warns_for_an_unmerged_branch_without_ready() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("app");
        std::fs::create_dir(&repo).unwrap();
        run(&repo, &["init", "-q", "-b", "main"]);
        run(&repo, &["commit", "-q", "--allow-empty", "-m", "a"]);
        let repo = std::path::PathBuf::from(run(&repo, &["rev-parse", "--show-toplevel"]));
        let main = repo.display().to_string();
        let feature = dir.path().join("feature");
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature",
                feature.to_str().unwrap(),
            ],
        );
        let feature = run(&feature, &["rev-parse", "--show-toplevel"]);
        let never = |_: &str, _: &str| Ok(false);
        // 원래 체크아웃의 브랜치는 보지 않는다. 대상과 같은 커밋(병합할 것이 없음)도 보지 않는다.
        assert_eq!(branch_warning(&repo, &main, never).await, Ok(None));
        assert_eq!(branch_warning(&repo, &feature, never).await, Ok(None));
        run(
            Path::new(&feature),
            &["commit", "-q", "--allow-empty", "-m", "b"],
        );
        let head = run(Path::new(&feature), &["rev-parse", "HEAD"]);
        let warning = branch_warning(&repo, &feature, never)
            .await
            .unwrap()
            .unwrap();
        assert!(
            warning.starts_with(&format!(
                "branch feature ({}) is not merged into main",
                &head[..8]
            )),
            "{warning}"
        );
        // 이 커밋으로 ready를 보냈으면 경고하지 않는다.
        let sent = |branch: &str, at: &str| Ok(branch == "feature" && at == head);
        assert_eq!(branch_warning(&repo, &feature, sent).await, Ok(None));
        // 병합한 뒤에는 경고하지 않는다.
        run(&repo, &["merge", "-q", "--no-ff", "-m", "m", "feature"]);
        assert_eq!(branch_warning(&repo, &feature, never).await, Ok(None));
    }

    #[tokio::test]
    async fn ownership_follows_the_registered_worktree_or_the_callers_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("app");
        std::fs::create_dir(&repo).unwrap();
        run(&repo, &["init", "-q", "-b", "main"]);
        run(&repo, &["commit", "-q", "--allow-empty", "-m", "a"]);
        let repo = std::path::PathBuf::from(run(&repo, &["rev-parse", "--show-toplevel"]));
        let main = repo.display().to_string();
        let feature = dir.path().join("feature");
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature",
                feature.to_str().unwrap(),
            ],
        );
        let feature = std::path::PathBuf::from(run(&feature, &["rev-parse", "--show-toplevel"]));
        let other = dir.path().join("other");
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "other",
                other.to_str().unwrap(),
            ],
        );
        let other = std::path::PathBuf::from(run(&other, &["rev-parse", "--show-toplevel"]));
        std::fs::create_dir(feature.join("src")).unwrap();
        let feature_dir = feature.join("src").display().to_string();

        // 등록한 워크트리가 그 브랜치다.
        let own = owned_worktree(&repo, &feature.display().to_string(), "feature", &main).await;
        assert_eq!(own.unwrap().branch.as_deref(), Some("feature"));
        // 원래 체크아웃의 노드가 그 브랜치 워크트리 안에서 부른다.
        let moved = owned_worktree(&repo, &main, "feature", &feature_dir)
            .await
            .unwrap();
        assert_eq!(moved.path, feature.display().to_string());
        // 그 브랜치 워크트리 안에 다른 브랜치의 워크트리가 중첩돼도 거절한다.
        let nested = feature.join("nested");
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "inner",
                nested.to_str().unwrap(),
            ],
        );
        let nested = nested.display().to_string();
        let refused = owned_worktree(&repo, &main, "feature", &nested).await;
        assert!(matches!(refused, Err(Refusal::NotOwner { .. })));
        // 작업 폴더가 다른 브랜치의 워크트리면 거절한다.
        let refused = owned_worktree(&repo, &main, "feature", &other.display().to_string()).await;
        assert_eq!(
            refused.unwrap_err(),
            Refusal::NotOwner {
                branch: "feature".into(),
                yours: Some("main".into())
            }
        );
    }
}
