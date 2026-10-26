use std::path::Path;
use std::sync::Arc;

use super::App;
use crate::git;
use crate::graph::{NodeId, Reclaim};
use crate::worktree::merge::{delete_merged, dirty, still_merged};
use crate::worktree::{self, RepoId, RepoProblem};

pub(super) async fn run(app: Arc<App>) {
    let rounds = match app.graph.reclaim_rounds() {
        Ok(rounds) => rounds,
        Err(e) => return eprintln!("loom: reclaim: {e}"),
    };
    for (repo, branch) in rounds {
        let done = match app.repos.path(repo) {
            Ok(dir) => reclaim(&app, &dir, repo, &branch).await,
            Err(e) => Err(e.to_string()),
        };
        if let Err(e) = done {
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
    let own = match worktree::find(dir, &path).await {
        Ok(found) => Some(found).filter(|w| !w.original),
        Err(RepoProblem::NotInRepo(_)) => None,
        Err(e) => return Err(e.to_string()),
    };
    let alone = |app: &App, but: Option<NodeId>| -> Result<bool, String> {
        Ok(app
            .graph
            .nodes_in(&path)
            .map_err(text)?
            .iter()
            .all(|n| Some(*n) == but))
    };
    let only_owner = match &own {
        Some(w) if alone(app, Some(round.owner))? => Some(w),
        _ => None,
    };
    if !still_merged(dir, only_owner, branch, &round.target).await? {
        return app
            .graph
            .note_reclaim(&round, &moved(branch, &round))
            .map_err(text);
    }
    if only_owner.is_some() && dirty(&path).await? {
        return app
            .graph
            .note_reclaim(&round, &kept(branch, &round))
            .map_err(text);
    }
    remove(app, round.owner).map_err(text)?;
    if let Some(own) = own
        && alone(app, None)?
        && let Err(e) = worktree::remove_clean(dir, &own).await
    {
        return app
            .graph
            .note_reclaim(&round, &changed(branch, &round, &e.to_string()))
            .map_err(text);
    }
    if let Some(cause) = delete_merged(dir, branch, &round.target).await? {
        app.graph
            .note_reclaim(&round, &remote_kept(branch, &cause))
            .map_err(text)?;
    }
    Ok(())
}

fn remove(app: &App, node: NodeId) -> rusqlite::Result<()> {
    if app.graph.remove_childless_child(node)? {
        app.host.kill(node);
    }
    Ok(())
}

fn kept(branch: &str, round: &Reclaim) -> String {
    format!(
        "{branch} merged, but the worktree {} has uncommitted changes. loom kept the worktree, the branch {branch} and its owner node. Commit or discard the changes, then remove the worktree, the branch and the node.",
        round.worktree
    )
}

fn moved(branch: &str, round: &Reclaim) -> String {
    format!(
        "{branch} merged, but the worktree {} is now on another branch or {branch} has commits that {} does not have. loom kept the worktree, the branch {branch} and its owner node.",
        round.worktree, round.target
    )
}

fn remote_kept(branch: &str, cause: &str) -> String {
    format!(
        "{branch} merged, but origin/{branch} changed after loom last fetched it, so loom kept it: {cause}. Delete it when its new commits are merged or not needed."
    )
}

fn changed(branch: &str, round: &Reclaim, cause: &str) -> String {
    format!(
        "{branch} merged and loom removed its owner node, but git did not remove the worktree {}: {cause}. loom kept the worktree and the branch {branch}. Commit or discard the changes, then remove the worktree and the branch.",
        round.worktree
    )
}
