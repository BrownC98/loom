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
    /// 손으로 쓴 메시지(`send`·`reply`·`done`)가 관문 낱말로 시작하면 거절한다. 관문 스크립트가 있는 저장소에서만 본다.
    /// 그런 저장소가 없는 동안(ktasearch) 옛 꼴을 그대로 쓴다(이전 기간).
    pub(super) fn gate_word(&self, node: NodeId, body: &str) -> Result<(), Refusal> {
        let Some((word, verb)) = protocol::gate_word(body) else {
            return Ok(());
        };
        if !self.gated_repo(node)? {
            return Ok(());
        }
        Err(Refusal::GateWord {
            word: word.to_owned(),
            verb: verb.to_owned(),
        })
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
                self.turn_ends.gated(node);
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

    async fn ready(&self, node: NodeId, branch: &str, via: &Via) -> Result<Readied, Refusal> {
        let place = self.graph.place_of(node)?;
        let repo = self.repo_of(node)?;
        let mine = worktree::find(&repo, &place.worktree)
            .await
            .map_err(git_problem)?;
        if mine.branch.as_deref() != Some(branch) {
            return Err(Refusal::NotOwner {
                branch: branch.to_owned(),
                yours: mine.branch,
            });
        }
        if !worktree::changes(&mine)
            .await
            .map_err(git_problem)?
            .is_empty()
        {
            return Err(Refusal::Uncommitted(branch.to_owned()));
        }
        if !self.gated_repo(node)? {
            return Err(Refusal::NoGateScripts);
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
        // 관문 에이전트는 WSL에서 이 경로를 읽는다. Windows 데몬이 보는 UNC 글자 대신 배포판 안의 경로를 쓴다.
        let worktree = crate::wsl::location(&place.worktree).map_or(place.worktree, |w| w.path);
        let round = Round {
            branch,
            target: &target,
            worktree: &worktree,
            head: &mine.head,
        };
        let gate = self.graph.ready(node, &round, gate, Some(via))?;
        Ok(Readied { gate, target })
    }

    /// `loom merge` 앞 단계: 상태를 `Merging`으로 바꾸고 audit를 돌린다. 찾은 일이 있으면 병합하지 않고 끝낸다.
    /// 부른 CLI는 이 앞에서 저장소의 병합 잠금을 잡았다. 그래서 `Merging`에서 다시 부르면 앞 실행은 이미 끝났다.
    async fn merge(&self, node: NodeId, branch: &str, via: &Via) -> Result<MergePlan, Refusal> {
        let repo = self.repo_of(node)?;
        if !self.gated_repo(node)? {
            return Err(Refusal::NoGateScripts);
        }
        let task = self.graph.merge_start(node, branch, Some(via))?;
        if task.from == GateState::Merging
            && let Ok(()) = contains(&repo, &task.head, &task.target).await
        {
            let range = format!("{}..{}", task.head, task.target);
            let first = git::run(
                &repo,
                &[
                    "rev-list",
                    "--first-parent",
                    "--ancestry-path",
                    "--reverse",
                    &range,
                ],
            )
            .await
            .map_err(Refusal::Git)?;
            let sha = first.lines().next().unwrap_or_default().trim().to_owned();
            let lines = vec![
                "recovered: an earlier run merged this commit, and its report did not arrive"
                    .to_owned(),
            ];
            self.record_merged(node, branch, &task, &sha, lines, via)
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
        if let Some(end) = protocol::audit_end(&findings) {
            let shown = findings.iter().map(Finding::to_string).collect();
            let ended = Ended {
                end,
                merged: None,
                lines: Vec::new(),
                findings,
            };
            self.graph
                .merge_end(node, branch, task.run, ended, Some(via))?;
            return Ok(MergePlan::Stopped {
                end,
                findings: shown,
            });
        }
        let into = if task.target == base_branch(&repo).await? {
            MergeInto::Main {
                checkout: repo.to_string_lossy().into_owned(),
            }
        } else {
            let found = worktree::worktrees(&repo)
                .await
                .into_iter()
                .find(|w| w.branch.as_deref() == Some(&task.target))
                .ok_or_else(|| Refusal::Git(format!("no worktree has {}", task.target)))?;
            MergeInto::Branch {
                worktree: found.path,
            }
        };
        Ok(MergePlan::Go {
            worktree: task.worktree,
            head: task.head,
            run: task.run,
            into,
        })
    }

    /// 병합 커밋 `sha`를 git에서 확인하고 `Merged`로 남긴다. 대상 브랜치가 원래 체크아웃의 브랜치면
    /// 앞선 관문 병합과 이 병합 사이의 대상 커밋을 관문 밖 커밋으로 붙인다(막지 않는다).
    async fn record_merged(
        &self,
        node: NodeId,
        branch: &str,
        task: &MergeTask,
        sha: &str,
        lines: Vec<String>,
        via: &Via,
    ) -> Result<GateState, Refusal> {
        let repo = self.repo_of(node)?;
        let not_merged = || Refusal::NotMerged {
            sha: sha.to_owned(),
            target: task.target.clone(),
            head: task.head.clone(),
        };
        if sha.is_empty() {
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
        if task.target == base_branch(&repo).await?
            && let Some(last) = self.graph.last_merged(node, &task.target)?
        {
            let range = format!("{last}..{sha}^1");
            let listed = git::run(&repo, &["rev-list", "--first-parent", &range])
                .await
                .map_err(Refusal::Git)?;
            findings = protocol::out_of_gate(&listed);
        }
        let ended = Ended {
            end: MergeEnd::Merged,
            merged: Some(sha),
            lines,
            findings,
        };
        self.graph
            .merge_end(node, branch, task.run, ended, Some(via))
    }

    /// `loom merge`를 부른 CLI의 끝 보고. 병합이면 git에서 확인한다.
    async fn merged(
        &self,
        node: NodeId,
        req: &MergedRequest,
        via: &Via,
    ) -> Result<GateState, Refusal> {
        let (end, cause, log) = match &req.result {
            MergeResult::Merged { sha, deploy, log } => {
                let task = self.graph.merge_task(node, &req.branch)?;
                if task.run != req.run {
                    return Err(Refusal::StaleRun {
                        branch: req.branch.clone(),
                        run: req.run,
                    });
                }
                let lines = vec![format!("deploy: {deploy:?}"), format!("log: {log}")];
                return self
                    .record_merged(node, &req.branch, &task, sha, lines, via)
                    .await;
            }
            MergeResult::Rework { cause, log } => (MergeEnd::Rework, cause, log),
            MergeResult::Failed { cause, log } => (MergeEnd::Failed, cause, log),
        };
        let ended = Ended {
            end,
            merged: None,
            lines: vec![format!("cause: {cause}"), format!("log: {log}")],
            findings: Vec::new(),
        };
        self.graph
            .merge_end(node, &req.branch, req.run, ended, Some(via))
    }
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

fn git_problem(e: worktree::RepoProblem) -> Refusal {
    Refusal::Git(e.to_string())
}

pub(super) async fn agent_ready(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<BranchRequest>,
) -> Result<Json<Readied>, AgentError> {
    let node = app.caller(&headers)?;
    let via = via(&headers, "/api/agent/ready");
    let result = app.ready(node, &req.branch, &via).await;
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
