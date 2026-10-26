use std::path::Path;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

use super::{AgentError, App, for_agent};
use crate::graph::{GateChoice, NodeId, Round, Via};
use crate::protocol::{self, CHECK_SCRIPT, GateState, GateVerb, Judgment, Refusal, SHIP_SCRIPT};
use crate::worktree;
use crate::worktree::merge::{base_branch, contains};

pub const TOOL_HEADER: &str = "loom-tool";
pub const VERSION_HEADER: &str = "loom-version";

#[derive(Serialize, Deserialize)]
pub struct ReadyRequest {
    pub branch: String,
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
    pub head: String,
    pub judgment: Judgment,
    pub findings: Option<String>,
}

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

    pub(super) async fn unreadied(&self, node: NodeId) -> Result<Option<String>, Refusal> {
        let place = self.graph.place_of(node)?;
        let repo = self.repo_of(node)?;
        if Path::new(&place.worktree) == repo
            || matches!(self.graph.gate_for(node)?, GateChoice::Parent { .. })
        {
            return Ok(None);
        }
        branch_warning(&repo, &place.worktree, |branch, head| {
            self.graph.ready_sent(node, branch, head)
        })
        .await
    }

    pub(super) fn gated_repo(&self, node: NodeId) -> Result<bool, Refusal> {
        let repo = self.repo_of(node)?;
        Ok(repo.join(CHECK_SCRIPT).is_file() && repo.join(SHIP_SCRIPT).is_file())
    }

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

    async fn gate_of(&self, node: NodeId, repo: &Path) -> Result<(NodeId, String), Refusal> {
        match self.graph.gate_for(node)? {
            GateChoice::Parent { gate, worktree } => {
                Ok((gate, target_of(repo, Some(&worktree)).await?))
            }
            GateChoice::Owner => Ok((node, target_of(repo, None).await?)),
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
        let (gate, target) = self.gate_of(node, &repo).await?;
        if target == branch {
            return Err(Refusal::SameAsTarget(branch.to_owned()));
        }
        if contains(&repo, branch, &target).await? {
            return Err(Refusal::NothingToMerge {
                branch: branch.to_owned(),
                target,
            });
        }
        let worktree =
            crate::wsl::location(&mine.path).map_or_else(|| mine.path.clone(), |w| w.path);
        let round = Round {
            branch,
            target: &target,
            worktree: &worktree,
            head: &mine.head,
        };
        let gate = match self.gated_repo(node)? {
            true => self.graph.ready(node, &round, Some(via))?,
            false => self.graph.announce_ready(node, &round, gate, Some(via))?,
        };
        Ok(Readied { gate, target })
    }
}

pub(super) async fn target_of(repo: &Path, gate_worktree: Option<&str>) -> Result<String, Refusal> {
    let base = base_branch(repo).await?;
    let Some(worktree) = gate_worktree else {
        return Ok(base);
    };
    match worktree::find(repo, worktree)
        .await
        .map_err(git_problem)?
        .branch
    {
        Some(branch) if branch != base => Ok(branch),
        branch => Err(Refusal::Git(format!(
            "the gate worktree {worktree} is {}; the gate must have its own branch there",
            branch.map_or_else(|| "detached".to_owned(), |b| format!("on {b}"))
        ))),
    }
}

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
    let cwd = Path::new(cwd);
    let top = worktree::top_of(cwd).await.map_err(Refusal::Git)?;
    worktree::worktrees(repo)
        .await
        .into_iter()
        .find(|w| w.branch.as_deref() == Some(branch) && w.path == top)
        .ok_or(Refusal::NotOwner {
            branch: branch.to_owned(),
            yours: mine.branch,
        })
}

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
    if contains(repo, &mine.head, &base).await? || ready_sent(&branch, &mine.head)? {
        return Ok(None);
    }
    Ok(Some(format!(
        "branch {branch} ({}) is not merged into {base}, and no `loom ready {branch}` was sent for this commit",
        short(&mine.head)
    )))
}

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
        req.head.trim(),
        req.judgment,
        req.findings.as_deref(),
        Some(&via),
    );
    if result == Ok(GateState::Passed) {
        app.merge_queue.notify_one();
    }
    app.traced(node, GateVerb::Verdict, &req.branch, &via, result)
}
