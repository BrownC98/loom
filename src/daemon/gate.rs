use std::path::Path;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

use super::verbs::{role_refusal, spawn};
use super::{AgentError, App, Problem, SpawnRequest, SpawnRun, for_agent};
use crate::graph::{GateChoice, NodeId, Round, Via};
use crate::protocol::{self, Deploy, GateState, GateVerb, Judgment, Refusal};
use crate::roles::REVIEWER_ROLES;
use crate::worktree;
use crate::worktree::RepoId;
use crate::worktree::merge::{base_branch, contains};

pub const TOOL_HEADER: &str = "loom-tool";
pub const VERSION_HEADER: &str = "loom-version";

#[derive(Serialize, Deserialize)]
pub struct ReadyRequest {
    pub branch: String,
    pub cwd: String,
    #[serde(default)]
    pub deploy: Deploy,
}

#[derive(Serialize, Deserialize)]
pub struct Readied {
    pub gate: String,
    pub target: String,
    #[serde(default)]
    pub unseated: Vec<String>,
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
    pub(super) fn kind_word(&self, body: &str) -> Result<(), Refusal> {
        let Some(found) = protocol::kind_word(body) else {
            return Ok(());
        };
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

    pub(super) async fn gate_of(
        &self,
        node: NodeId,
        repo: &Path,
    ) -> Result<(NodeId, String), Refusal> {
        match self.graph.gate_for(node)? {
            GateChoice::Parent { gate, worktree } => {
                Ok((gate, target_of(repo, Some(&worktree)).await?))
            }
            GateChoice::Owner => Ok((node, target_of(repo, None).await?)),
        }
    }

    pub(super) fn parent_changed(self: &Arc<Self>, owners: &[NodeId]) {
        for &owner in owners {
            self.run_unless_busy(owner);
        }
        let app = self.clone();
        let owners = owners.to_vec();
        tokio::spawn(async move {
            for owner in owners {
                if let Err(e) = app.tell_new_gate(owner).await {
                    eprintln!("loom: gate of node {}: {e}", owner.0);
                }
            }
        });
    }

    async fn tell_new_gate(self: &Arc<Self>, owner: NodeId) -> Result<(), Refusal> {
        let rounds = self.graph.waiting_rounds(owner)?;
        if rounds.is_empty() {
            return Ok(());
        }
        let gate_worktree = match self.graph.gate_for(owner)? {
            GateChoice::Parent { worktree, .. } => Some(worktree),
            GateChoice::Owner => None,
        };
        for (repo, branch) in rounds {
            if gate_worktree.is_none() {
                let problems = match self.unseated_roles(owner, &branch) {
                    Ok(roles) => self.seat_reviewers(owner, &branch, roles).await,
                    Err(e) => vec![e.to_string()],
                };
                for problem in problems {
                    eprintln!("loom: reviewer of {branch}: {problem}");
                }
            }
            if let Err(e) = self
                .tell_round(repo, &branch, gate_worktree.as_deref())
                .await
            {
                eprintln!("loom: gate of {branch}: {e}");
            }
        }
        Ok(())
    }

    async fn tell_round(
        &self,
        repo: RepoId,
        branch: &str,
        gate_worktree: Option<&str>,
    ) -> Result<(), Refusal> {
        let dir = self
            .repos
            .path(repo)
            .map_err(|e| Refusal::Git(e.to_string()))?;
        let target = target_of(&dir, gate_worktree).await?;
        Ok(self.graph.tell_gate(repo, branch, &target)?)
    }

    fn unseated_roles(&self, gate: NodeId, branch: &str) -> Result<Vec<&'static str>, Refusal> {
        let roles = self.settings.get()?.roles;
        let mut unseated = Vec::new();
        for role in REVIEWER_ROLES {
            if self.graph.reviewer_seated(gate, branch, role)? {
                continue;
            }
            roles.assign(role, "").map_err(role_refusal)?;
            unseated.push(role);
        }
        Ok(unseated)
    }

    async fn seat_reviewers(
        self: &Arc<Self>,
        gate: NodeId,
        branch: &str,
        roles: Vec<&str>,
    ) -> Vec<String> {
        let mut problems = Vec::new();
        for role in roles {
            let request = SpawnRequest {
                run: SpawnRun::Review {
                    role: role.to_owned(),
                    branch: branch.to_owned(),
                },
                brief: String::new(),
                worktree: None,
            };
            match spawn(self, gate, request).await {
                Ok(_) | Err(Problem::Refused(Refusal::ReviewerSeated { .. })) => {}
                Err(e) => problems.push(format!("{role}: {}", for_agent(e).1)),
            }
        }
        problems
    }

    fn repo_of(&self, node: NodeId) -> Result<std::path::PathBuf, Refusal> {
        let place = self.graph.place_of(node)?;
        self.repos
            .path(place.repo)
            .map_err(|e| Refusal::Git(e.to_string()))
    }

    async fn ready(
        self: &Arc<Self>,
        node: NodeId,
        branch: &str,
        deploy: Deploy,
        cwd: &str,
        via: &Via,
    ) -> Result<Readied, Refusal> {
        let place = self.graph.place_of(node)?;
        let repo = self.repo_of(node)?;
        let mine = owned_worktree(&repo, &place.worktree, branch, cwd).await?;
        if worktree::dirty(Path::new(&mine.path))
            .await
            .map_err(git_problem)?
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
        let worktree = crate::wsl::linux_of(&mine.path);
        let round = Round {
            branch,
            target: &target,
            worktree: &worktree,
            head: &mine.head,
            deploy,
        };
        let roles = match gate == node {
            true => self.unseated_roles(gate, branch)?,
            false => Vec::new(),
        };
        let gate_name = match self.graph.ready(node, &round, Some(via)) {
            Err(Refusal::SameHead(_)) if !roles.is_empty() => self.graph.name(gate)?,
            opened => opened?,
        };
        let unseated = self.seat_reviewers(gate, branch, roles).await;
        Ok(Readied {
            gate: gate_name,
            target,
            unseated,
        })
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
    let result = app
        .ready(node, &req.branch, req.deploy, &req.cwd, &via)
        .await;
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
