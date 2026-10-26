use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::App;
use super::gate::target_of;
use crate::git;
use crate::graph::{BatchId, BatchMember, BatchResult, Ended, Entry, Merge, Outcome, Queued};
use crate::protocol::{self, Finding, Refusal, SHIP_DEPLOY_FAILED};
use crate::worktree::merge::{
    self, ADVANCE, CHECK_FAILED, CLEAN, CONFLICT, DIRTY, Lines, MOVED, PUBLISH, STACK, SYNC, UNDO,
    base_branch, bringing_merge, merge_commit, script,
};
use crate::worktree::{self, RepoId};

const MOVE_LIMIT: u32 = 3;

const CHECK_LOCK: &str = "/tmp/loom-gate-check.lock";

pub(super) async fn run(app: Arc<App>) {
    let mut changes = app.graph.changes();
    tokio::spawn(super::reclaim::run(app.clone()));
    loop {
        recover(&app, CHECK_LOCK).await;
        'next: loop {
            let gates = match app.graph.queued_gates() {
                Ok(gates) => gates,
                Err(e) => {
                    eprintln!("loom: merge queue: {e}");
                    break;
                }
            };
            for (repo, gate) in gates {
                match batch(&app, repo, gate.as_deref(), CHECK_LOCK).await {
                    Ok(true) => continue 'next,
                    Ok(false) => {}
                    Err(e) => eprintln!("loom: merge queue: {e}"),
                }
            }
            break;
        }
        tokio::select! {
            () = app.merge_queue.notified() => {}
            changed = changes.recv() => if matches!(changed, Err(tokio::sync::broadcast::error::RecvError::Closed)) {
                return;
            },
        }
    }
}

async fn batch(
    app: &Arc<App>,
    repo: RepoId,
    gate: Option<&str>,
    lock: &str,
) -> Result<bool, Refusal> {
    let dir = repo_path(app, repo)?;
    let target = target_of(&dir, gate).await?;
    let Some((id, queue)) = app.graph.batch_open(repo, gate, &target)? else {
        return Ok(false);
    };
    let run = Batch {
        app,
        id,
        repo,
        dir,
        target,
        lock,
    };
    if let Err(e) = run.run(queue.clone()).await {
        fail_all(app, repo, id, &queue, &e.to_string());
    }
    app.graph.batch_ended(id)?;
    Ok(true)
}

async fn recover(app: &Arc<App>, lock: &str) {
    let stale = match app.graph.stale_batches() {
        Ok(stale) => stale,
        Err(e) => return eprintln!("loom: merge queue recovery: {e}"),
    };
    for stale in stale {
        let dir = match repo_path(app, stale.repo) {
            Ok(dir) => dir,
            Err(e) => {
                fail_all(app, stale.repo, stale.id, &stale.members, &e.to_string());
                if let Err(e) = app.graph.batch_close(stale.id) {
                    eprintln!("loom: merge queue recovery: {e}");
                }
                continue;
            }
        };
        let run = Batch {
            app,
            id: stale.id,
            repo: stale.repo,
            dir,
            target: stale.target,
            lock,
        };
        if let Err(e) = script(&run.dir, CLEAN, &[]).await {
            eprintln!("loom: merge queue recovery: {e}");
        }
        run.record(0, &stale.members, None, BatchResult::Interrupted, None);
        let done = match (stale.base, stale.tip) {
            (Some(base), Some(tip)) if !stale.members.is_empty() => {
                run.resume(&stale.members, &base, &tip).await
            }
            _ => {
                stale.members.iter().for_each(|m| run.requeue(m));
                Ok(())
            }
        };
        if let Err(e) = done {
            fail_all(app, stale.repo, stale.id, &stale.members, &e.to_string());
        }
        if let Err(e) = app.graph.batch_close(stale.id) {
            eprintln!("loom: merge queue recovery: {e}");
        }
    }
}

fn fail_all(app: &App, repo: RepoId, id: BatchId, members: &[Queued], cause: &str) {
    for m in members {
        let ended = failed(
            format!("the merge queue stopped: {cause}"),
            None,
            batch_lines(id),
        );
        let _ = app.graph.merge_end(repo, &m.branch, ended);
    }
}

fn batch_lines(id: BatchId) -> Vec<String> {
    vec![format!("batch: {}", id.0)]
}

fn failed(cause: String, log: Option<String>, lines: Vec<String>) -> Ended<'static> {
    Ended {
        outcome: Outcome::Failed,
        cause: Some(cause),
        log,
        lines,
        findings: Vec::new(),
    }
}

fn source_of(repo: &Path, member: &Queued) -> PathBuf {
    PathBuf::from(git::from_git_path(repo, &member.worktree))
}

fn repo_path(app: &App, repo: RepoId) -> Result<PathBuf, Refusal> {
    app.repos
        .path(repo)
        .map_err(|e| Refusal::Git(e.to_string()))
}

struct Batch<'a> {
    app: &'a Arc<App>,
    id: BatchId,
    repo: RepoId,
    dir: PathBuf,
    target: String,
    lock: &'a str,
}

enum Dest {
    Base,
    Coordinator { worktree: String },
}

enum Trial {
    Conflict { index: usize, log: Option<String> },
    Failed { log: Option<String> },
    Passed { tip: String },
}

impl Batch<'_> {
    async fn run(&self, queue: Vec<Queued>) -> Result<(), Refusal> {
        let mut members = Vec::new();
        for member in queue {
            if self.screen(&member).await? {
                members.push(member);
            }
        }
        if members.is_empty() {
            return Ok(());
        }
        let into = self.into().await?;
        match &into {
            Dest::Base => {
                let (code, out) =
                    script(&self.dir, SYNC, std::slice::from_ref(&self.target)).await?;
                if code != 0 {
                    return Err(Refusal::Git(out.trim().to_owned()));
                }
            }
            Dest::Coordinator { worktree } => {
                let found = worktree::find(&self.dir, worktree)
                    .await
                    .map_err(|e| Refusal::Git(e.to_string()))?;
                if worktree::dirty(Path::new(&found.path))
                    .await
                    .map_err(|e| Refusal::Git(e.to_string()))?
                {
                    self.dirty(&members, worktree);
                    return Ok(());
                }
            }
        }
        let base = merge::target_base(&self.dir, &self.target).await?;
        self.bisect(members, base.trim(), &into).await
    }

    async fn bisect(&self, members: Vec<Queued>, base: &str, into: &Dest) -> Result<(), Refusal> {
        let mut prefix: Vec<Queued> = Vec::new();
        let mut suspects = members;
        let mut known_bad = false;
        let mut failed_log = None;
        let mut step = 0;
        loop {
            let half = match known_bad {
                true if suspects.len() == 1 => {
                    let culprit = suspects.remove(0);
                    self.settle(&culprit, self.rework(self.check_cause(step), failed_log));
                    prefix.iter().for_each(|m| self.requeue(m));
                    return Ok(());
                }
                true => suspects.len() / 2,
                false => suspects.len(),
            };
            let trial: Vec<Queued> = prefix.iter().chain(&suspects[..half]).cloned().collect();
            match self.trial(&trial, base, step).await? {
                Trial::Conflict { index, log } => {
                    let member = trial[index].clone();
                    self.settle(&member, self.rework(self.conflict_cause(), log));
                    prefix.retain(|m| m.branch != member.branch);
                    suspects.retain(|m| m.branch != member.branch);
                    known_bad = false;
                    if prefix.is_empty() && suspects.is_empty() {
                        return Ok(());
                    }
                    if suspects.is_empty() {
                        suspects = std::mem::take(&mut prefix);
                    }
                    continue;
                }
                Trial::Passed { tip } if half == suspects.len() => {
                    let branches: Vec<&str> = trial.iter().map(|m| m.branch.as_str()).collect();
                    let checked = self
                        .app
                        .graph
                        .batch_checked(self.id, base, &tip, &branches)?;
                    if !self.unchanged(&trial).await? || !checked {
                        return Ok(());
                    }
                    return self.publish_into(&trial, base, &tip, step, into).await;
                }
                Trial::Passed { .. } => {
                    prefix.extend(suspects.drain(..half));
                }
                Trial::Failed { log } if half == suspects.len() => {
                    known_bad = true;
                    failed_log = log;
                }
                Trial::Failed { log } => {
                    failed_log = log;
                    suspects.drain(half..).for_each(|m| self.requeue(&m));
                }
            }
            step += 1;
        }
    }

    async fn trial(&self, trial: &[Queued], base: &str, step: u32) -> Result<Trial, Refusal> {
        let mut args = vec![
            format!("batch-{}-{step}", self.id.0),
            self.lock.to_owned(),
            base.to_owned(),
        ];
        for m in trial {
            args.push(m.branch.clone());
            args.push(m.head.clone());
        }
        let (code, out) = script(&self.dir, STACK, &args).await?;
        let lines = Lines::read(&out);
        let (found, result) = match (code, lines.conflict, &lines.tip) {
            (CONFLICT, Some(index), _) if index < trial.len() => (
                Trial::Conflict {
                    index,
                    log: lines.log.clone(),
                },
                BatchResult::Conflict {
                    branch: trial[index].branch.clone(),
                },
            ),
            (CHECK_FAILED, _, _) => (
                Trial::Failed {
                    log: lines.log.clone(),
                },
                BatchResult::CheckFailed,
            ),
            (0, _, Some(tip)) => (Trial::Passed { tip: tip.clone() }, BatchResult::Passed),
            _ => {
                let cause = format!("the batch script exited {code}: {}", out.trim());
                self.record(
                    step,
                    trial,
                    lines.check,
                    BatchResult::Error {
                        cause: cause.clone(),
                    },
                    lines.log,
                );
                return Err(Refusal::Git(cause));
            }
        };
        self.record(step, trial, lines.check, result, lines.log);
        Ok(found)
    }

    async fn publish_into(
        &self,
        members: &[Queued],
        base: &str,
        tip: &str,
        step: u32,
        into: &Dest,
    ) -> Result<(), Refusal> {
        match into {
            Dest::Base => self.publish_base(members, base, tip, step).await,
            Dest::Coordinator { worktree } => {
                let (code, out) = script(
                    Path::new(worktree),
                    ADVANCE,
                    &[self.target.clone(), tip.to_owned()],
                )
                .await?;
                match code {
                    0 => self.merged(members, base, tip, None, into).await,
                    DIRTY => {
                        self.dirty(members, worktree);
                        Ok(())
                    }
                    MOVED => self.moved(members, step),
                    _ => Err(Refusal::Git(format!(
                        "fast-forward of {} exited {code}: {}",
                        self.target,
                        out.trim()
                    ))),
                }
            }
        }
    }

    async fn resume(&self, members: &[Queued], base: &str, tip: &str) -> Result<(), Refusal> {
        let into = self.into().await?;
        let done = match &into {
            Dest::Base => {
                merge::fetch(&self.dir, &self.target).await?;
                format!("refs/remotes/origin/{}", self.target)
            }
            Dest::Coordinator { .. } => format!("refs/heads/{}", self.target),
        };
        let published = merge::published(&self.dir, tip, &done).await;
        if !published {
            if let Dest::Base = into {
                let args = [self.target.clone(), base.to_owned(), tip.to_owned()];
                let (code, out) = script(&self.dir, UNDO, &args).await?;
                if code != 0 {
                    return Err(Refusal::Git(format!(
                        "{} needs repair: {} (batch base {base}, tip {tip}, not on origin)",
                        self.target,
                        out.trim()
                    )));
                }
            }
            if !self.unchanged(members).await? {
                return Ok(());
            }
        }
        self.publish_into(members, base, tip, 0, &into).await
    }

    async fn screen(&self, member: &Queued) -> Result<bool, Refusal> {
        let findings = self.audit(member).await?;
        if !findings.iter().any(Finding::stops) {
            return Ok(true);
        }
        self.settle(
            member,
            Ended {
                outcome: Outcome::Rework,
                cause: None,
                log: None,
                lines: self.lines(),
                findings,
            },
        );
        Ok(false)
    }

    async fn unchanged(&self, members: &[Queued]) -> Result<bool, Refusal> {
        let held = self.app.graph.batch_members(self.id)?;
        let mut kept = Vec::new();
        for m in members {
            let Some((_, gate)) = held.iter().find(|(branch, _)| *branch == m.branch) else {
                continue;
            };
            if target_of(&self.dir, gate.as_deref()).await.ok().as_deref()
                != Some(self.target.as_str())
            {
                self.requeue(m);
            } else if self.screen(m).await? {
                kept.push(m);
            }
        }
        if kept.len() == members.len() {
            return Ok(true);
        }
        kept.into_iter().for_each(|m| self.requeue(m));
        Ok(false)
    }

    async fn publish_base(
        &self,
        members: &[Queued],
        base: &str,
        tip: &str,
        step: u32,
    ) -> Result<(), Refusal> {
        let mut last = None;
        for m in members.iter().rev() {
            if let Some(sha) = merge_commit(&self.dir, &m.head, tip).await? {
                last = Some((m, sha));
                break;
            }
        }
        let Some((last, merged)) = last else {
            return self.merged(members, base, tip, None, &Dest::Base).await;
        };
        let args = [
            format!("batch-{}-publish.log", self.id.0),
            self.target.clone(),
            base.to_owned(),
            tip.to_owned(),
            last.branch.clone(),
            merged,
        ];
        let (code, out) = script(&self.dir, PUBLISH, &args).await?;
        let lines = Lines::read(&out);
        let log = lines.log;
        match (code, lines.published) {
            (0, _) => {
                self.merged(
                    members,
                    base,
                    tip,
                    Some("deploy: confirmed".into()),
                    &Dest::Base,
                )
                .await
            }
            (SHIP_DEPLOY_FAILED, _) => {
                self.merged(
                    members,
                    base,
                    tip,
                    Some("deploy: failed (see the log)".into()),
                    &Dest::Base,
                )
                .await
            }
            (_, true) => {
                let deploy = format!(
                    "deploy: unknown ({} exited {code}; see {})",
                    protocol::SHIP_SCRIPT,
                    log.as_deref().unwrap_or("the publish log")
                );
                self.merged(members, base, tip, Some(deploy), &Dest::Base)
                    .await
            }
            (MOVED, false) => self.moved(members, step),
            (_, false) => {
                let cause =
                    format!("publishing the batch exited {code} (see the last lines of the log)");
                for m in members {
                    self.settle(m, self.failed(cause.clone(), log.clone()));
                }
                Ok(())
            }
        }
    }

    async fn merged(
        &self,
        members: &[Queued],
        base: &str,
        tip: &str,
        deploy: Option<String>,
        into: &Dest,
    ) -> Result<(), Refusal> {
        let settled = self.settle_merged(members, base, tip, deploy, into).await;
        tokio::spawn(super::reclaim::run(self.app.clone()));
        settled
    }

    async fn settle_merged(
        &self,
        members: &[Queued],
        base: &str,
        tip: &str,
        deploy: Option<String>,
        into: &Dest,
    ) -> Result<(), Refusal> {
        let into_base = matches!(into, Dest::Base);
        for m in members {
            let own = merge_commit(&self.dir, &m.head, tip).await?;
            let sha = match own {
                Some(sha) => Some(sha),
                None => bringing_merge(&self.dir, &m.head, base, tip).await?,
            };
            let Some(sha) = sha else {
                self.settle(
                    m,
                    self.failed(
                        format!(
                            "{} was already in {} before the batch; nothing was merged",
                            m.head, self.target
                        ),
                        None,
                    ),
                );
                continue;
            };
            let mut findings: Vec<Finding> = self
                .app
                .graph
                .audit(m.owner, m.since)?
                .into_iter()
                .filter(|f| !f.stops())
                .collect();
            if into_base {
                let known = self.app.graph.gate_merges(self.repo, &self.target)?;
                let listed = merge::first_parents(&self.dir, &sha).await?;
                findings.extend(protocol::out_of_gate(&listed, &known));
            }
            let mut lines = self.lines();
            lines.extend(deploy.clone());
            self.settle(
                m,
                Ended {
                    outcome: Outcome::Merged(Merge {
                        sha: &sha,
                        target: &self.target,
                    }),
                    cause: None,
                    log: None,
                    lines,
                    findings,
                },
            );
        }
        Ok(())
    }

    async fn audit(&self, member: &Queued) -> Result<Vec<Finding>, Refusal> {
        let mut findings = self.app.graph.audit(member.owner, member.since)?;
        if let Ok(now) = merge::branch_head(&self.dir, &member.branch).await
            && now.trim() != member.head
        {
            findings.push(Finding::HeadMoved {
                reviewed: member.head.clone(),
                now: now.trim().to_owned(),
            });
        }
        let source = source_of(&self.dir, member);
        if source.is_dir() && !merge::clean_at(&source, &member.head).await {
            findings.push(Finding::SourceDirty {
                worktree: member.worktree.clone(),
            });
        }
        Ok(findings)
    }

    async fn into(&self) -> Result<Dest, Refusal> {
        if self.target == base_branch(&self.dir).await? {
            return Ok(Dest::Base);
        }
        worktree::worktrees(&self.dir)
            .await
            .into_iter()
            .find(|w| w.branch.as_deref() == Some(self.target.as_str()))
            .map(|w| Dest::Coordinator { worktree: w.path })
            .ok_or_else(|| Refusal::Git(format!("no worktree has {}", self.target)))
    }

    fn dirty(&self, members: &[Queued], worktree: &str) {
        self.record(0, members, None, BatchResult::TargetDirty, None);
        for m in members {
            self.settle(
                m,
                self.rework(
                    format!(
                        "the coordinator worktree {} has uncommitted changes; the coordinator commits them, then the owner calls loom ready again",
                        crate::wsl::location(worktree).map_or_else(|| worktree.to_owned(), |w| w.path)
                    ),
                    None,
                ),
            );
        }
    }

    fn moved(&self, members: &[Queued], step: u32) -> Result<(), Refusal> {
        self.record(step, members, None, BatchResult::TargetMoved, None);
        if self.app.graph.target_moves(self.repo, &self.target)? + 1 < MOVE_LIMIT {
            members.iter().for_each(|m| self.requeue(m));
            return Ok(self.app.graph.batch_moved(self.id)?);
        }
        let cause = format!(
            "{} moved during {MOVE_LIMIT} batches in a row; tell the root, then send loom ready again",
            self.target
        );
        for m in members {
            self.settle(m, self.failed(cause.clone(), None));
        }
        Ok(())
    }

    fn settle(&self, member: &Queued, ended: Ended) {
        if let Err(e) = self.app.graph.merge_end(self.repo, &member.branch, ended) {
            eprintln!("loom: merge queue {}: {e}", member.branch);
        }
    }

    fn requeue(&self, member: &Queued) {
        if let Err(e) = self.app.graph.requeue(self.repo, &member.branch) {
            eprintln!("loom: merge queue {}: {e}", member.branch);
        }
    }

    fn record(
        &self,
        step: u32,
        members: &[Queued],
        check_secs: Option<u64>,
        result: BatchResult,
        log: Option<String>,
    ) {
        let entry = Entry::Batch {
            batch: self.id.0,
            repo: self.dir.display().to_string(),
            target: self.target.clone(),
            step,
            members: members
                .iter()
                .map(|m| BatchMember {
                    branch: m.branch.clone(),
                    head: m.head.clone(),
                    waited_secs: m.waited,
                })
                .collect(),
            check_secs,
            result,
            log,
        };
        if let Err(e) = self.app.graph.record(&entry) {
            eprintln!("loom: merge queue trace: {e}");
        }
    }

    fn lines(&self) -> Vec<String> {
        batch_lines(self.id)
    }

    fn rework(&self, cause: String, log: Option<String>) -> Ended<'static> {
        Ended {
            outcome: Outcome::Rework,
            cause: Some(cause),
            log,
            lines: self.lines(),
            findings: Vec::new(),
        }
    }

    fn failed(&self, cause: String, log: Option<String>) -> Ended<'static> {
        failed(cause, log, self.lines())
    }

    fn conflict_cause(&self) -> String {
        format!(
            "the branch conflicts with {} and the branches queued before it; merge {} into the branch, fix the conflict, commit, then loom ready again",
            self.target, self.target
        )
    }

    fn check_cause(&self, step: u32) -> String {
        format!(
            "{} failed with this branch on {} (batch {}, bisect step {step}); fix it, commit, then loom ready again",
            protocol::CHECK_SCRIPT,
            self.target,
            self.id.0
        )
    }
}
