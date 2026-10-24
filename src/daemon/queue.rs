use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::App;
use super::gate::{base_branch, merge_commit};
use crate::git;
use crate::graph::{BatchId, BatchMember, BatchResult, Ended, Entry, Queued, Refusal};
use crate::protocol::{self, Finding, MergeEnd, SHIP_DEPLOY_FAILED};
use crate::worktree::{self, RepoId};

const TICK: Duration = Duration::from_secs(60);

const CONFLICT: i32 = 10;
const CHECK_FAILED: i32 = 14;
const MOVED: i32 = 20;
const DIRTY: i32 = 21;
const MOVE_LIMIT: u32 = 3;

const CHECK_LOCK: &str = "/tmp/loom-gate-check.lock";

const CLEAN: &str = r#"common=$(git rev-parse --path-format=absolute --git-common-dir)
for wt in "$common"/loom-batches/*; do
  [ -e "$wt" ] || continue
  git worktree remove --force "$wt" 2>/dev/null
  rm -rf "$wt"
done
git worktree prune
"#;

const SYNC: &str = r#"set -u
target=$1
[ "$(git symbolic-ref --quiet --short HEAD)" = "$target" ] || { echo "the main checkout is not on $target"; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "the main checkout has uncommitted changes"; exit 1; }
git fetch --quiet origin "$target" || { echo "git fetch origin $target failed"; exit 1; }
git merge --ff-only --quiet "origin/$target" || { echo "$target and origin/$target diverged"; exit 1; }
[ "$(git rev-parse HEAD)" = "$(git rev-parse "origin/$target")" ] || { echo "$target has commits that are not on origin/$target"; exit 1; }
"#;

const STACK: &str = r#"set -u
common=$(git rev-parse --path-format=absolute --git-common-dir)
wt=$common/loom-batches/$1 log=$common/loom-logs/$1.log lock=$2 base=$3
shift 3
mkdir -p "$common/loom-logs" "$common/loom-batches"
exec 3>&1 >>"$log" 2>&1
echo "log $log" >&3
clean() { git worktree remove --force "$wt" 2>/dev/null; rm -rf "$wt"; git worktree prune; }
clean
trap clean EXIT
git worktree add --detach "$wt" "$base" || exit 1
i=0
while [ $# -gt 0 ]; do
  if git -C "$wt" merge-base --is-ancestor "$2" HEAD; then
    echo "== $1 $2 is already in the batch"
  else
    echo "== merge $1 $2"
    git -C "$wt" merge --no-ff -m "Merge branch '$1'" "$2" || { git -C "$wt" merge --abort; echo "conflict $i" >&3; exit 10; }
  fi
  i=$((i + 1))
  shift 2
done
tip=$(git -C "$wt" rev-parse HEAD)
exec 9>>"$lock"
flock 9
start=$(date +%s)
(cd "$wt" && CARGO_TARGET_DIR="$common/loom-batch-target" bash scripts/gate/check.sh 9>&-)
code=$?
echo "check $(($(date +%s) - start))" >&3
[ "$code" -eq 0 ] || exit 14
echo "tip $tip" >&3
"#;

const PUBLISH: &str = r#"set -u
common=$(git rev-parse --path-format=absolute --git-common-dir)
log=$common/loom-logs/$1 target=$2 base=$3 tip=$4
mkdir -p "$common/loom-logs"
exec 3>&1 >>"$log" 2>&1
echo "log $log" >&3
[ "$(git symbolic-ref --quiet --short HEAD)" = "$target" ] || { echo "the main checkout is not on $target"; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "the main checkout has uncommitted changes"; exit 1; }
git fetch --quiet origin "$target" || { echo "git fetch failed"; exit 1; }
if ! git merge-base --is-ancestor "$tip" "origin/$target"; then
  [ "$(git rev-parse "origin/$target")" = "$base" ] || { echo "origin/$target moved"; exit 20; }
  head=$(git rev-parse HEAD)
  [ "$head" = "$base" ] || [ "$head" = "$tip" ] || { echo "$target moved"; exit 20; }
  git merge --ff-only --quiet "$tip" || exit 1
  [ "$(git rev-parse HEAD)" = "$tip" ] || { echo "$target moved after the fast-forward to $tip"; exit 1; }
  if ! git push --quiet origin "$tip:refs/heads/$target"; then
    [ -z "$(git status --porcelain --untracked-files=no)" ] && git update-ref "refs/heads/$target" "$base" "$tip" ||
      { echo "$target moved during git push; loom kept it and did not undo the fast-forward to $tip"; exit 1; }
    git reset --hard --quiet
    git fetch --quiet origin "$target" || { echo "git fetch failed after git push failed"; exit 1; }
    [ "$(git rev-parse "origin/$target")" = "$base" ] || { echo "origin/$target moved during git push"; exit 20; }
    echo "git push origin $target failed"
    exit 1
  fi
fi
echo published >&3
exec bash scripts/gate/ship.sh "$5" "$6"
"#;

const UNDO: &str = r#"set -u
target=$1 base=$2 tip=$3
git merge-base --is-ancestor "$tip" "refs/heads/$target" || exit 0
[ "$(git symbolic-ref --quiet --short HEAD)" = "$target" ] || { echo "the main checkout is not on $target"; exit 1; }
[ -z "$(git status --porcelain --untracked-files=no)" ] || { echo "the main checkout has uncommitted changes"; exit 1; }
git update-ref "refs/heads/$target" "$base" "$tip" || { echo "$target moved past the batch tip"; exit 1; }
git reset --hard --quiet
"#;

const ADVANCE: &str = r#"set -u
[ "$(git symbolic-ref --quiet --short HEAD)" = "$1" ] || { echo "the coordinator worktree is not on $1"; exit 1; }
[ -z "$(git status --porcelain)" ] || exit 21
git merge --ff-only --quiet "$2" || exit 20
"#;

pub(super) async fn run(app: Arc<App>) {
    let mut moves: HashMap<(RepoId, String), u32> = HashMap::new();
    loop {
        recover(&app, CHECK_LOCK).await;
        loop {
            let next = match app.graph.queued_targets() {
                Ok(targets) => targets.into_iter().next(),
                Err(e) => {
                    eprintln!("loom: merge queue: {e}");
                    None
                }
            };
            let Some((repo, target)) = next else { break };
            let key = (repo, target);
            let before = moves.get(&key).copied().unwrap_or(0);
            match batch(&app, key.0, &key.1, CHECK_LOCK, before).await {
                Ok(true) => {
                    moves.insert(key, before + 1);
                }
                Ok(false) => {
                    moves.remove(&key);
                }
                Err(e) => {
                    eprintln!("loom: merge queue {}: {e}", key.1);
                    break;
                }
            }
        }
        tokio::select! {
            () = app.merge_queue.notified() => {}
            () = tokio::time::sleep(TICK) => {}
        }
    }
}

async fn batch(
    app: &App,
    repo: RepoId,
    target: &str,
    lock: &str,
    moves: u32,
) -> Result<bool, Refusal> {
    let queue = app.graph.queue(repo, target)?;
    let branches: Vec<&str> = queue.iter().map(|q| q.branch.as_str()).collect();
    let dir = repo_path(app, repo);
    let id = app.graph.batch_open(repo, target, &branches)?;
    match dir {
        Ok(dir) => {
            let run = Batch {
                app,
                id,
                repo,
                dir,
                target: target.to_owned(),
                lock,
                moves,
                moved: AtomicBool::new(false),
            };
            if let Err(e) = run.run(queue.clone()).await {
                fail_all(app, repo, id, &queue, &e.to_string());
            }
            app.graph.batch_close(id)?;
            Ok(run.moved.load(Ordering::Relaxed))
        }
        Err(e) => {
            fail_all(app, repo, id, &queue, &e.to_string());
            app.graph.batch_close(id)?;
            Ok(false)
        }
    }
}

async fn recover(app: &App, lock: &str) {
    match app.graph.orphans() {
        Ok(orphans) => {
            for (repo, branch) in orphans {
                let ended = failed(UPGRADE_CAUSE.to_owned(), None, Vec::new());
                if let Err(e) = app.graph.merge_end(repo, &branch, ended) {
                    eprintln!("loom: merge queue recovery {branch}: {e}");
                }
            }
        }
        Err(e) => eprintln!("loom: merge queue recovery: {e}"),
    }
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
            moves: 0,
            moved: AtomicBool::new(false),
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

const UPGRADE_CAUSE: &str = "loom upgrade: send loom ready again";

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
        end: MergeEnd::Failed,
        merged: None,
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
    app: &'a App,
    id: BatchId,
    repo: RepoId,
    dir: PathBuf,
    target: String,
    lock: &'a str,
    moves: u32,
    moved: AtomicBool,
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
                if !worktree::changes(&found)
                    .await
                    .map_err(|e| Refusal::Git(e.to_string()))?
                    .is_empty()
                {
                    self.dirty(&members, worktree);
                    return Ok(());
                }
            }
        }
        let base = git::run(
            &self.dir,
            &[
                "rev-parse",
                "--verify",
                &format!("refs/heads/{}^{{commit}}", self.target),
            ],
        )
        .await
        .map_err(Refusal::Git)?;
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
                    if !self.unchanged(&trial).await? {
                        return Ok(());
                    }
                    self.app.graph.batch_checked(self.id, base, &tip)?;
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
                    0 => self.merged(members, base, tip, None).await,
                    DIRTY => {
                        self.dirty(members, worktree);
                        Ok(())
                    }
                    MOVED => {
                        self.moved(members, step);
                        Ok(())
                    }
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
                git::run(&self.dir, &["fetch", "--quiet", "origin", &self.target])
                    .await
                    .map_err(Refusal::Git)?;
                format!("refs/remotes/origin/{}", self.target)
            }
            Dest::Coordinator { .. } => format!("refs/heads/{}", self.target),
        };
        let published = git::run(&self.dir, &["merge-base", "--is-ancestor", tip, &done])
            .await
            .is_ok();
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
        let Some(end) = protocol::audit_end(&findings) else {
            return Ok(true);
        };
        self.settle(
            member,
            Ended {
                end,
                merged: None,
                cause: None,
                log: None,
                lines: self.lines(),
                findings,
            },
        );
        Ok(false)
    }

    async fn unchanged(&self, members: &[Queued]) -> Result<bool, Refusal> {
        let mut kept = Vec::new();
        for m in members {
            if self.screen(m).await? {
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
            if merge_commit(&self.dir, &m.head, tip).await?.is_some() {
                last = Some(m);
                break;
            }
        }
        let Some(last) = last else {
            return self.merged(members, base, tip, None).await;
        };
        let args = [
            format!("batch-{}-publish.log", self.id.0),
            self.target.clone(),
            base.to_owned(),
            tip.to_owned(),
            last.branch.clone(),
            last.head.clone(),
        ];
        let (code, out) = script(&self.dir, PUBLISH, &args).await?;
        let lines = Lines::read(&out);
        let log = lines.log;
        match (code, lines.published) {
            (0, _) => {
                self.merged(members, base, tip, Some("deploy: confirmed".into()))
                    .await
            }
            (SHIP_DEPLOY_FAILED, _) => {
                self.merged(
                    members,
                    base,
                    tip,
                    Some("deploy: failed (see the log)".into()),
                )
                .await
            }
            (_, true) => {
                let deploy = format!(
                    "deploy: unknown ({} exited {code}; see {})",
                    protocol::SHIP_SCRIPT,
                    log.as_deref().unwrap_or("the publish log")
                );
                self.merged(members, base, tip, Some(deploy)).await
            }
            (MOVED, false) => {
                self.moved(members, step);
                Ok(())
            }
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
    ) -> Result<(), Refusal> {
        let into_base = base_branch(&self.dir).await? == self.target;
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
            let mut findings = Vec::new();
            if into_base {
                let known = self.app.graph.gate_merges(self.repo, &self.target)?;
                let listed = git::run(
                    &self.dir,
                    &["rev-list", "--first-parent", &format!("{sha}^1")],
                )
                .await
                .map_err(Refusal::Git)?;
                findings = protocol::out_of_gate(&listed, &known);
            }
            let mut lines = self.lines();
            lines.extend(deploy.clone());
            self.settle(
                m,
                Ended {
                    end: MergeEnd::Merged,
                    merged: Some(&sha),
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
        if let Ok(now) = git::run(
            &self.dir,
            &[
                "rev-parse",
                "--verify",
                &format!("refs/heads/{}", member.branch),
            ],
        )
        .await
            && now.trim() != member.head
        {
            findings.push(Finding::HeadMoved {
                reviewed: member.head.clone(),
                now: now.trim().to_owned(),
            });
        }
        let source = source_of(&self.dir, member);
        if source.is_dir() {
            let head = git::run(&source, &["rev-parse", "HEAD"]).await;
            let changes =
                git::run(&source, &["status", "--porcelain", "--untracked-files=no"]).await;
            match (head, changes) {
                (Ok(head), Ok(changes))
                    if head.trim() == member.head && changes.trim().is_empty() => {}
                _ => findings.push(Finding::SourceDirty {
                    worktree: member.worktree.clone(),
                }),
            }
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

    fn moved(&self, members: &[Queued], step: u32) {
        self.record(step, members, None, BatchResult::TargetMoved, None);
        if self.moves + 1 < MOVE_LIMIT {
            self.moved.store(true, Ordering::Relaxed);
            return members.iter().for_each(|m| self.requeue(m));
        }
        let cause = format!(
            "{} moved during {MOVE_LIMIT} batches in a row; tell the root, then send loom ready again",
            self.target
        );
        for m in members {
            self.settle(m, self.failed(cause.clone(), None));
        }
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
            end: MergeEnd::Rework,
            merged: None,
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

async fn bringing_merge(
    dir: &Path,
    head: &str,
    base: &str,
    tip: &str,
) -> Result<Option<String>, Refusal> {
    let listed = git::run(
        dir,
        &[
            "rev-list",
            "--first-parent",
            "--reverse",
            &format!("{base}..{tip}"),
        ],
    )
    .await
    .map_err(Refusal::Git)?;
    for sha in listed.lines() {
        if super::gate::contains(dir, head, sha).await? {
            return Ok(Some(sha.to_owned()));
        }
    }
    Ok(None)
}

#[derive(Default, Debug, PartialEq)]
struct Lines {
    log: Option<String>,
    conflict: Option<usize>,
    check: Option<u64>,
    tip: Option<String>,
    published: bool,
}

impl Lines {
    fn read(out: &str) -> Lines {
        let mut lines = Lines::default();
        for line in out.lines() {
            let mut words = line.split_whitespace();
            match (words.next(), words.next(), words.next()) {
                (Some("log"), _, _) => lines.log = line.strip_prefix("log ").map(str::to_owned),
                (Some("conflict"), Some(i), _) => lines.conflict = i.parse().ok(),
                (Some("check"), Some(secs), _) => lines.check = secs.parse().ok(),
                (Some("tip"), Some(tip), _) => lines.tip = Some(tip.to_owned()),
                (Some("published"), _, _) => lines.published = true,
                _ => {}
            }
        }
        lines
    }
}

async fn script(dir: &Path, text: &str, args: &[String]) -> Result<(i32, String), Refusal> {
    let mut cmd = crate::wsl::command(&dir.to_string_lossy(), "bash");
    cmd.arg("-c").arg(text).arg("loom-queue").args(args);
    cmd.stdin(std::process::Stdio::null());
    let out = tokio::process::Command::from(cmd)
        .output()
        .await
        .map_err(|e| Refusal::Git(format!("could not run bash: {e}")))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.code() != Some(0) {
        text.push_str(&String::from_utf8_lossy(&out.stderr));
    }
    Ok((out.status.code().unwrap_or(-1), text))
}

#[cfg(test)]
#[path = "queue_tests.rs"]
mod tests;
