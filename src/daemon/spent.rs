use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::App;
use crate::graph::{Kept, NodeId};
use crate::worktree::merge::{delete_local_merged, still_merged};
use crate::worktree::{self, RepoProblem, Worktree};

#[derive(Default)]
pub(super) struct Swept {
    pub removed: Vec<String>,
    pub kept: Vec<String>,
}

enum Left {
    Nothing,
    Clean {
        dir: PathBuf,
        own: Worktree,
        branch: String,
        target: String,
    },
    Work(String),
}

impl App {
    pub(super) async fn sweep(self: &Arc<Self>, parent: NodeId, keep: &[Kept]) -> Swept {
        let mut swept = Swept::default();
        let mut failed = Vec::new();
        if let Err(e) = self.graph.accept_done(parent, keep) {
            swept
                .kept
                .push(format!("loom could not mark finished children: {e}"));
        }
        loop {
            let spent = match self.graph.spent_children(parent, keep) {
                Ok(spent) => spent,
                Err(e) => {
                    swept
                        .kept
                        .push(format!("loom could not list finished children: {e}"));
                    break;
                }
            };
            let spent: Vec<_> = spent
                .into_iter()
                .filter(|(id, _)| !failed.contains(id))
                .collect();
            if spent.is_empty() {
                break;
            }
            for (child, name) in spent {
                match self.sweep_one(parent, child, &name, keep, &mut swept).await {
                    Ok(true) => swept.removed.push(name),
                    Ok(false) => failed.push(child),
                    Err(e) => {
                        failed.push(child);
                        swept
                            .kept
                            .push(format!("{name}: loom could not remove it: {e}"));
                    }
                }
            }
        }
        match self.graph.accepted_in_turn(parent) {
            Ok(names) => swept.kept.extend(names.into_iter().map(|name| {
                format!("{name}: its turn is still running. loom removes it when the turn ends.")
            })),
            Err(e) => swept
                .kept
                .push(format!("loom could not list children in a turn: {e}")),
        }
        swept
    }

    pub(super) fn remove_accepted(self: &Arc<Self>, child: NodeId) {
        let accepted = match self.graph.accepted_by(child) {
            Ok(Some(accepted)) => accepted,
            Ok(None) => return,
            Err(e) => {
                eprintln!("loom: 노드 {}의 완료 수락을 읽지 못했다: {e}", child.0);
                return;
            }
        };
        let app = self.clone();
        tokio::spawn(async move {
            let (parent, name) = accepted;
            let mut swept = Swept::default();
            if let Err(e) = app.sweep_one(parent, child, &name, &[], &mut swept).await {
                swept.kept.push(format!("loom could not remove it: {e}"));
            }
            for kept in swept.kept {
                eprintln!("loom: 턴이 끝난 노드 {} ({name}): {kept}", child.0);
            }
        });
    }

    async fn sweep_one(
        self: &Arc<Self>,
        parent: NodeId,
        child: NodeId,
        name: &str,
        keep: &[Kept],
        swept: &mut Swept,
    ) -> Result<bool, String> {
        let left = self.left(child).await?;
        let limits = self.settings.get().map_err(|e| e.to_string())?.tree_limits;
        let Some(moved) = self
            .graph
            .remove_spent_child(parent, child, name, keep, limits)
            .map_err(|e| e.to_string())?
        else {
            return Ok(false);
        };
        self.host.kill(child);
        let moved: Vec<NodeId> = moved.into_iter().map(|(id, _)| id).collect();
        self.parent_changed(&moved);
        match left {
            Left::Nothing => {}
            Left::Work(why) => swept.kept.push(format!("{name}: {why}")),
            Left::Clean {
                dir,
                own,
                branch,
                target,
            } => {
                if let Err(e) = worktree::remove_clean(&dir, &own).await {
                    swept.kept.push(format!(
                        "{name}: git did not remove the worktree {}: {e}. loom kept it and the branch {branch}.",
                        own.path
                    ));
                    return Ok(true);
                }
                if let Err(e) = delete_local_merged(&dir, &branch, &target).await {
                    swept.kept.push(format!(
                        "{name}: git did not delete the branch {branch}: {e}. loom kept it."
                    ));
                }
            }
        }
        Ok(true)
    }

    async fn left(&self, child: NodeId) -> Result<Left, String> {
        let place = self.graph.place_of(child).map_err(|e| e.to_string())?;
        let dir = self.repos.path(place.repo).map_err(|e| e.to_string())?;
        if Path::new(&place.worktree) == dir {
            return Ok(Left::Nothing);
        }
        let shared = self
            .graph
            .nodes_in(&place.worktree)
            .map_err(|e| e.to_string())?
            .iter()
            .any(|n| *n != child);
        if shared {
            return Ok(Left::Nothing);
        }
        let own = match worktree::find(&dir, &place.worktree).await {
            Ok(own) if !own.original => own,
            Ok(_) | Err(RepoProblem::NotInRepo(_)) => return Ok(Left::Nothing),
            Err(e) => return Err(e.to_string()),
        };
        let path = own.path.clone();
        if worktree::dirty(Path::new(&path))
            .await
            .map_err(|e| e.to_string())?
        {
            return Ok(Left::Work(format!(
                "the worktree {path} has uncommitted changes. loom kept it and its branch."
            )));
        }
        let Some(branch) = own.branch.clone() else {
            return Ok(Left::Work(format!(
                "the worktree {path} has a detached HEAD. loom kept it."
            )));
        };
        let target = match self.gate_of(child, &dir).await {
            Ok((_, target)) => target,
            Err(e) => {
                return Ok(Left::Work(format!(
                    "loom did not find the target of {branch} ({e}). loom kept the worktree {path} and the branch."
                )));
            }
        };
        if !still_merged(&dir, Some(Some(&branch)), &branch, &target).await? {
            return Ok(Left::Work(format!(
                "the branch {branch} has commits that {target} does not have. loom kept the worktree {path} and the branch."
            )));
        }
        Ok(Left::Clean {
            dir,
            own,
            branch,
            target,
        })
    }
}
