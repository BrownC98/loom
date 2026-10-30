use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::App;
use crate::graph::NodeId;
use crate::worktree::merge::{delete_local_merged, still_merged};
use crate::worktree::{self, RepoProblem, Worktree};

#[derive(Default)]
pub(super) struct Swept {
    pub removed: Vec<String>,
    pub kept: Vec<String>,
    pub problems: Vec<String>,
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
    pub(super) async fn sweep(self: &Arc<Self>, parent: NodeId) -> Swept {
        let mut swept = Swept::default();
        let mut failed = Vec::new();
        loop {
            let spent = match self.graph.spent_children(parent) {
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
                match self.sweep_one(parent, child, &name, &mut swept).await {
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
        match self.graph.finishing_children(parent) {
            Ok(names) => swept.kept.extend(names.into_iter().map(|name| {
                format!("{name}: its turn is still running. loom removes it when the turn ends.")
            })),
            Err(e) => swept
                .kept
                .push(format!("loom could not list children in a turn: {e}")),
        }
        if !swept.removed.is_empty() {
            self.sweep_above(parent);
        }
        swept
    }

    pub(super) fn sweep_above(self: &Arc<Self>, node: NodeId) {
        match self.graph.parent(node) {
            Ok(Some(parent)) => self.sweep_later(parent),
            Ok(None) => {}
            Err(e) => eprintln!("loom: 노드 {}의 부모를 읽지 못했다: {e}", node.0),
        }
    }

    pub(super) fn sweep_later(self: &Arc<Self>, parent: NodeId) {
        let app = self.clone();
        tokio::spawn(async move {
            for problem in app.sweep(parent).await.problems {
                let body = format!("loom removed a finished child of yours. {problem}");
                if let Err(e) = app.graph.note_from_loom(parent, &body) {
                    eprintln!("loom: 노드 {}에 회수 알림을 보내지 못했다: {e}", parent.0);
                }
            }
        });
    }

    async fn sweep_one(
        self: &Arc<Self>,
        parent: NodeId,
        child: NodeId,
        name: &str,
        swept: &mut Swept,
    ) -> Result<bool, String> {
        if self.graph.parent_holds(child).map_err(|e| e.to_string())?
            && let Left::Work(_) = self.work(parent).await?
        {
            swept.kept.push(format!(
                "{name}: the branch of its parent is not merged yet. loom keeps it for gate rework and removes it after the merge."
            ));
            return Ok(false);
        }
        let left = self.left(child).await?;
        if let Left::Work(why) = left {
            swept.kept.push(format!("{name}: {why}"));
            return Ok(false);
        }
        let limits = self.settings.get().map_err(|e| e.to_string())?.tree_limits;
        let Some(moved) = self
            .graph
            .remove_spent_child(parent, child, name, limits)
            .map_err(|e| e.to_string())?
        else {
            return Ok(false);
        };
        self.host.kill(child);
        let moved: Vec<NodeId> = moved.into_iter().map(|(id, _)| id).collect();
        self.parent_changed(&moved);
        if let Left::Clean {
            dir,
            own,
            branch,
            target,
        } = left
        {
            if let Err(e) = worktree::remove_clean(&dir, &own).await {
                swept.problems.push(format!(
                    "{name}: git did not remove the worktree {}: {e}. loom kept it and the branch {branch}.",
                    own.path
                ));
                return Ok(true);
            }
            if let Err(e) = delete_local_merged(&dir, &branch, &target).await {
                swept.problems.push(format!(
                    "{name}: git did not delete the branch {branch}: {e}. loom kept it."
                ));
            }
        }
        Ok(true)
    }

    async fn left(&self, child: NodeId) -> Result<Left, String> {
        let place = self.graph.place_of(child).map_err(|e| e.to_string())?;
        let shared = self
            .graph
            .nodes_in(&place.worktree)
            .map_err(|e| e.to_string())?
            .iter()
            .any(|n| *n != child);
        if shared {
            return Ok(Left::Nothing);
        }
        self.work(child).await
    }

    async fn work(&self, node: NodeId) -> Result<Left, String> {
        let place = self.graph.place_of(node).map_err(|e| e.to_string())?;
        let dir = self.repos.path(place.repo).map_err(|e| e.to_string())?;
        if Path::new(&place.worktree) == dir {
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
                "the worktree {path} has uncommitted changes. loom keeps the node, the worktree and its branch."
            )));
        }
        let Some(branch) = own.branch.clone() else {
            return Ok(Left::Work(format!(
                "the worktree {path} has a detached HEAD. loom keeps the node and the worktree."
            )));
        };
        let target = match self.gate_of(node, &dir).await {
            Ok((_, target)) => target,
            Err(e) => {
                return Ok(Left::Work(format!(
                    "loom did not find the target of {branch} ({e}). loom keeps the node, the worktree {path} and the branch."
                )));
            }
        };
        if !still_merged(&dir, Some(Some(&branch)), &branch, &target).await? {
            return Ok(Left::Work(format!(
                "the branch {branch} has commits that {target} does not have. loom keeps the node, the worktree {path} and the branch until they are merged."
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
