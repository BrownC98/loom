use rusqlite::params;

use super::gate::{exists, root_of};
use super::{Graph, NodeId, mailbox};
use crate::protocol::{Kind, MessageKind};
use crate::worktree::RepoId;

#[derive(Clone, Debug, PartialEq)]
pub struct Deploy {
    pub id: i64,
    pub repo: RepoId,
    pub target: String,
    pub sha: String,
    pub branch: String,
}

impl Graph {
    pub fn deploy_started(
        &self,
        repo: RepoId,
        target: &str,
        sha: &str,
        branch: &str,
        owners: &[NodeId],
    ) -> rusqlite::Result<Option<Deploy>> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let mut roots = Vec::new();
        for owner in owners {
            if exists(&tx, *owner)? {
                let root = root_of(&tx, *owner)?;
                if !roots.contains(&root) {
                    roots.push(root);
                }
            }
        }
        let mut id = None;
        for root in roots {
            tx.execute(
                "INSERT INTO deploys (repo, target, sha, branch, root) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![repo.0, target, sha, branch, root.0],
            )?;
            id = Some(tx.last_insert_rowid());
        }
        tx.commit()?;
        Ok(id.map(|id| Deploy {
            id,
            repo,
            target: target.to_owned(),
            sha: sha.to_owned(),
            branch: branch.to_owned(),
        }))
    }

    pub fn deploys(&self) -> rusqlite::Result<Vec<Deploy>> {
        self.lock()
            .prepare(
                "SELECT id, repo, target, sha, branch FROM deploys d
                 WHERE id = (SELECT max(id) FROM deploys WHERE repo = d.repo AND target = d.target)
                 ORDER BY id",
            )?
            .query_map([], |r| {
                Ok(Deploy {
                    id: r.get(0)?,
                    repo: RepoId(r.get(1)?),
                    target: r.get(2)?,
                    sha: r.get(3)?,
                    branch: r.get(4)?,
                })
            })?
            .collect()
    }

    pub fn deploy_ended(&self, deploy: &Deploy, failure: Option<&str>) -> rusqlite::Result<()> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        let newer: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM deploys WHERE repo = ?1 AND target = ?2 AND id > ?3)",
            params![deploy.repo.0, deploy.target, deploy.id],
            |r| r.get(0),
        )?;
        if newer {
            return Ok(());
        }
        let covered: Vec<(String, NodeId)> = tx
            .prepare(
                "SELECT branch, root FROM deploys WHERE repo = ?1 AND target = ?2 AND id <= ?3 ORDER BY id",
            )?
            .query_map(params![deploy.repo.0, deploy.target, deploy.id], |r| {
                Ok((r.get(0)?, NodeId(r.get(1)?)))
            })?
            .collect::<rusqlite::Result<_>>()?;
        tx.execute(
            "DELETE FROM deploys WHERE repo = ?1 AND target = ?2 AND id <= ?3",
            params![deploy.repo.0, deploy.target, deploy.id],
        )?;
        let mut told = false;
        if let Some(failure) = failure {
            let mut branches: Vec<&str> = Vec::new();
            let mut roots = Vec::new();
            for (branch, root) in &covered {
                if !branches.contains(&branch.as_str()) {
                    branches.push(branch);
                }
                if !roots.contains(root) {
                    roots.push(*root);
                }
            }
            let body = format!(
                "deploy failed: {failure}\nmerges: {} into {} (up to {})\nNext: read the log and decide. The merges stay on {}; only the deploy failed.",
                branches.join(", "),
                deploy.target,
                deploy.sha,
                deploy.target
            );
            for root in roots {
                if exists(&tx, root)? {
                    mailbox::enqueue_from_loom(
                        &tx,
                        root,
                        Kind::Queue,
                        &MessageKind::Note,
                        &body,
                        None,
                    )?;
                    told = true;
                }
            }
        }
        tx.commit()?;
        if told {
            self.publish_nodes();
            self.mail.notify_waiters();
        }
        Ok(())
    }
}
