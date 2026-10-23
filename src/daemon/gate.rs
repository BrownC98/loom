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

pub const TOOL_HEADER: &str = "loom-tool";
pub const VERSION_HEADER: &str = "loom-version";

#[derive(Serialize, Deserialize)]
pub struct BranchRequest {
    pub branch: String,
}

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

#[derive(Serialize, Deserialize)]
pub enum MergePlan {
    Stopped {
        end: MergeEnd,
        findings: Vec<String>,
    },
    Recorded {
        sha: String,
    },
    Go {
        worktree: String,
        head: String,
        run: i64,
        into: MergeInto,
    },
}

#[derive(Serialize, Deserialize)]
pub enum MergeInto {
    Main { checkout: String },
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
    Rework {
        cause: String,
        log: String,
    },
    Failed {
        cause: String,
        log: String,
    },
    Stopped {
        cause: String,
        log: String,
    },
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq)]
pub enum Deploy {
    Confirmed,
    Failed,
    NotApplicable,
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
        if self.graph.under_coordinator(node)? {
            return Ok(None);
        }
        let place = self.graph.place_of(node)?;
        let repo = self.repo_of(node)?;
        if Path::new(&place.worktree) == repo {
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

async fn base_branch(repo: &Path) -> Result<String, Refusal> {
    worktree::worktrees(repo)
        .await
        .into_iter()
        .find(|w| w.original)
        .and_then(|w| w.branch)
        .ok_or_else(|| Refusal::Git("the main checkout is not on a branch".into()))
}

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

struct MergedNotes {
    lines: Vec<String>,
    log: Option<String>,
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
    if contains(repo, &mine.head, &base).await.is_ok() || ready_sent(&branch, &mine.head)? {
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
