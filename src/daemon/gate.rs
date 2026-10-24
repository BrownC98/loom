use std::path::Path;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

use super::{AgentError, App, for_agent};
use crate::git;
use crate::graph::{GateChoice, NodeId, Refusal, Round, Via};
use crate::protocol::{self, CHECK_SCRIPT, GateState, GateVerb, Judgment, SHIP_SCRIPT};
use crate::worktree;

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
    pub judgment: Judgment,
    pub findings: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct FailRequest {
    pub branch: String,
    pub cause: String,
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
        if Path::new(&place.worktree) == repo || self.gate_of(node, &repo).await?.0 != node {
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
        let base = base_branch(repo).await?;
        match self.graph.gate_for(node)? {
            GateChoice::Parent {
                gate,
                worktree,
                branched_from,
            } => {
                let from = match branched_from {
                    Some(branch) => Some(branch),
                    None => {
                        worktree::find(repo, &worktree)
                            .await
                            .map_err(git_problem)?
                            .branch
                    }
                };
                match from {
                    Some(target) if target != base => Ok((gate, target)),
                    _ => Ok((node, base)),
                }
            }
            GateChoice::Owner => Ok((node, base)),
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
            true => self.graph.ready(node, &round, gate, Some(via))?,
            false => self.graph.announce_ready(node, &round, gate, Some(via))?,
        };
        Ok(Readied { gate, target })
    }
}

pub(super) async fn merge_commit(
    repo: &Path,
    head: &str,
    tip: &str,
) -> Result<Option<String>, Refusal> {
    let listed = git::run(repo, &["log", "--first-parent", "--format=%H %P", tip])
        .await
        .map_err(Refusal::Git)?;
    Ok(listed.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let sha = parts.next()?;
        parts.skip(1).any(|p| p == head).then(|| sha.to_owned())
    }))
}

pub(super) async fn base_branch(repo: &Path) -> Result<String, Refusal> {
    worktree::worktrees(repo)
        .await
        .into_iter()
        .find(|w| w.original)
        .and_then(|w| w.branch)
        .ok_or_else(|| Refusal::Git("the main checkout is not on a branch".into()))
}

pub(super) async fn contains(repo: &Path, older: &str, newer: &str) -> Result<bool, Refusal> {
    match git::run(repo, &["merge-base", "--is-ancestor", older, newer]).await {
        Ok(_) => Ok(true),
        Err(e) if e.is_empty() => Ok(false),
        Err(e) => Err(Refusal::Git(e)),
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
        req.judgment,
        req.findings.as_deref(),
        Some(&via),
    );
    if result == Ok(GateState::Passed) {
        app.merge_queue.notify_one();
    }
    app.traced(node, GateVerb::Verdict, &req.branch, &via, result)
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
        let sent = |branch: &str, at: &str| Ok(branch == "feature" && at == head);
        assert_eq!(branch_warning(&repo, &feature, sent).await, Ok(None));
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

        let own = owned_worktree(&repo, &feature.display().to_string(), "feature", &main).await;
        assert_eq!(own.unwrap().branch.as_deref(), Some("feature"));
        let moved = owned_worktree(&repo, &main, "feature", &feature_dir)
            .await
            .unwrap();
        assert_eq!(moved.path, feature.display().to_string());
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
