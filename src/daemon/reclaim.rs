use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::App;
use super::gate::contains;
use crate::git;
use crate::graph::{NodeId, Reclaim};
use crate::worktree::{self, RepoId};

pub(super) async fn run(app: Arc<App>, repo: RepoId, branches: Vec<String>) {
    let dir = match app.repos.path(repo) {
        Ok(dir) => dir,
        Err(e) => return eprintln!("loom: reclaim: {e}"),
    };
    for branch in branches {
        if let Err(e) = reclaim(&app, &dir, repo, &branch).await {
            eprintln!("loom: reclaim {branch}: {e}");
        }
    }
}

async fn reclaim(app: &App, dir: &Path, repo: RepoId, branch: &str) -> Result<(), String> {
    let text = |e: rusqlite::Error| e.to_string();
    let Some(round) = app.graph.reclaim(repo, branch).map_err(text)? else {
        return Ok(());
    };
    for reviewer in &round.reviewers {
        remove(app, *reviewer).map_err(text)?;
    }
    let path = git::from_git_path(dir, &round.worktree);
    let own = worktree::find(dir, &path)
        .await
        .ok()
        .filter(|w| !w.original);
    let alone = |app: &App, but: Option<NodeId>| -> Result<bool, String> {
        Ok(app
            .graph
            .nodes_in(&path)
            .map_err(text)?
            .iter()
            .all(|n| Some(*n) == but))
    };
    if own.is_some() && alone(app, Some(round.owner))? && dirty(&path).await? {
        return app
            .graph
            .note_from_loom(round.told, &kept(branch, &round))
            .map_err(text);
    }
    remove(app, round.owner).map_err(text)?;
    if let Some(own) = own
        && alone(app, None)?
    {
        worktree::remove(dir, &own)
            .await
            .map_err(|e| e.to_string())?;
    }
    delete_branch(dir, branch, &round.target).await
}

fn remove(app: &App, node: NodeId) -> rusqlite::Result<()> {
    if app.graph.remove_childless_child(node)? {
        app.host.kill(node);
    }
    Ok(())
}

async fn dirty(path: &str) -> Result<bool, String> {
    let status = git::run(
        Path::new(path),
        &["status", "--porcelain", "--untracked-files=all"],
    )
    .await?;
    Ok(!status.trim().is_empty())
}

fn kept(branch: &str, round: &Reclaim) -> String {
    format!(
        "{branch} merged, but the worktree {} has uncommitted changes. loom kept the worktree, the branch {branch} and its owner node. Commit or discard the changes, then remove the worktree, the branch and the node.",
        round.worktree
    )
}

async fn delete_branch(dir: &Path, branch: &str, target: &str) -> Result<(), String> {
    let listed = worktree::worktrees(dir).await;
    let local = format!("refs/heads/{branch}");
    if listed.iter().any(|w| w.branch.as_deref() == Some(branch))
        || git::run(dir, &["rev-parse", "--verify", "--quiet", &local])
            .await
            .is_err()
    {
        return Ok(());
    }
    let at = listed
        .iter()
        .find(|w| w.branch.as_deref() == Some(target))
        .map_or_else(|| dir.to_path_buf(), |w| PathBuf::from(&w.path));
    let remote = format!("refs/remotes/origin/{branch}");
    let pushed = git::run(dir, &["rev-parse", "--verify", "--quiet", &remote])
        .await
        .is_ok();
    if pushed
        && contains(dir, &remote, &format!("refs/heads/{target}"))
            .await
            .map_err(|e| e.to_string())?
    {
        git::run(dir, &["push", "--quiet", "origin", "--delete", branch]).await?;
    }
    git::run(&at, &["branch", "-d", "--", branch]).await?;
    Ok(())
}
