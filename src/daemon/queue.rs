//! 병합 큐: 데몬의 작업 하나가 pass 판정을 받은 브랜치를 대상마다 묶어 쌓고, check.sh를 한 번 돌리고, 병합·push·배포 확인을 한다.
//! 에이전트는 기다리지 않는다. 결과는 merged·rework·failed 메시지로 간다(2026-10-23 사람 결정 Q697 A, 총괄 #3761:
//! 전경 `loom merge`가 12분 넘게 셸을 막아 그동안 메일이 쌓였다).
//! 작업이 하나라 이 데몬의 check.sh는 한 번에 하나다. 다른 데몬(시험 데몬)과는 스크립트의 `flock`이 가른다.
//! Windows 데몬은 스크립트를 저장소의 WSL 배포판 안에서 띄운다(`wsl::command`). 스크립트는 배포판 안의 경로만 쓰고, 데몬은 표준 출력의 결과 줄과 끝 코드만 읽는다.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::App;
use super::gate::{base_branch, merge_commit};
use crate::git;
use crate::graph::{BatchId, BatchMember, BatchResult, Ended, Entry, Queued, Refusal};
use crate::protocol::{self, Finding, MergeEnd, SHIP_DEPLOY_FAILED};
use crate::worktree::{self, RepoId};

/// pass 판정이 큐를 깨운다. 그래도 한 번씩 본다: 묶음이 DB 오류로 멈춘 뒤에도 큐가 멈추지 않는다.
const TICK: Duration = Duration::from_secs(60);

/// 스크립트 끝 코드. 스크립트 글의 `exit` 숫자와 같아야 한다(`tests::scripts_use_the_exit_codes`).
const CONFLICT: i32 = 10;
const CHECK_FAILED: i32 = 14;
/// 쌓은 뒤 대상이 움직였다(원격·코디 브랜치에 새 커밋, push 거절). 묶음은 큐에 다시 선다.
const MOVED: i32 = 20;
/// 대상 코디 워크트리에 미커밋 변경이 있다.
const DIRTY: i32 = 21;

/// check.sh는 VM 메모리 8GiB를 쓸 수 있다. 호스트에서 한 번에 하나만 돌게 이 파일을 `flock`으로 쥔다. 배포판 안의 경로다.
const CHECK_LOCK: &str = "/tmp/loom-gate-check.lock";

/// 저장소마다 묶음 임시 워크트리(`<git 공유 폴더>/loom-batch`)를 지운다. 데몬이 묶음 중에 끝났으면 남는다.
macro_rules! clean_fn {
    () => {
        r#"common=$(git rev-parse --path-format=absolute --git-common-dir)
clean() { git worktree remove --force "$common/loom-batch" 2>/dev/null; rm -rf "$common/loom-batch"; git worktree prune; }
"#
    };
}

const CLEAN: &str = concat!(clean_fn!(), "clean\n");

/// 원래 체크아웃의 대상(main)을 원격에 맞춘다. 묶음은 게시된 대상 위에 쌓는다. 인자: 대상.
const SYNC: &str = r#"set -u
target=$1
[ "$(git symbolic-ref --quiet --short HEAD)" = "$target" ] || { echo "the main checkout is not on $target"; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "the main checkout has uncommitted changes"; exit 1; }
git fetch --quiet origin "$target" || { echo "git fetch origin $target failed"; exit 1; }
git merge --ff-only --quiet "origin/$target" || { echo "$target and origin/$target diverged"; exit 1; }
"#;

/// 묶음 하나를 쌓고 검사한다. 시작 폴더는 원래 체크아웃이다. 인자: 기록 파일 이름, 대상, 검사 잠금(`CHECK_LOCK`), 그 뒤 (브랜치, 검토한 커밋) 쌍을 큐 순서로.
/// 임시 워크트리에서 대상 위에 `--no-ff`로 차례로 병합하고 check.sh를 한 번 돌린다. cargo 빌드는 공유 폴더(`loom-batch-target`)에 두어 다음 묶음이 다시 쓴다.
/// 잠금은 check.sh의 자식에게 넘기지 않는다:
/// 남은 자식이 잠금을 쥐면 다음 검사가 끝없이 기다린다(옛 `loom merge` 검토 P3-e).
/// 결과 줄(표준 출력): `log <경로>`, 충돌이면 `conflict <순번>`, 검사했으면 `check <초>`, 지나면 `tip <끝> <대상>`.
const STACK: &str = concat!(
    "set -u\n",
    clean_fn!(),
    r#"log=$common/loom-logs/$1 target=$2 lock=$3
shift 3
mkdir -p "$common/loom-logs"
exec 3>&1 >>"$log" 2>&1
echo "log $log" >&3
clean
trap clean EXIT
base=$(git rev-parse --verify "refs/heads/$target^{commit}") || exit 1
git worktree add --detach "$common/loom-batch" "$base" || exit 1
i=0
while [ $# -gt 0 ]; do
  echo "== merge $1 $2"
  git -C "$common/loom-batch" merge --no-ff -m "Merge branch '$1'" "$2" || { git -C "$common/loom-batch" merge --abort; echo "conflict $i" >&3; exit 10; }
  i=$((i + 1))
  shift 2
done
tip=$(git -C "$common/loom-batch" rev-parse HEAD)
exec 9>>"$lock"
flock 9
start=$(date +%s)
(cd "$common/loom-batch" && CARGO_TARGET_DIR="$common/loom-batch-target" bash scripts/gate/check.sh 9>&-)
code=$?
echo "check $(($(date +%s) - start))" >&3
[ "$code" -eq 0 ] || exit 14
echo "tip $tip $base" >&3
"#
);

/// 원래 체크아웃에 묶음을 게시한다. 다시 불러도 된다(데몬이 게시 중에 끝났을 때). 인자: 기록 파일 이름, 대상, 쌓은 대상 커밋, 묶음 끝, 마지막 브랜치와 그 커밋.
/// 묶음 끝이 이미 원격에 있으면 push를 건너뛴다. 아니면 대상을 묶음 끝으로 옮기고 push한다. 그 뒤 ship.sh가 이어 하기 길로 배포만 확인한다:
/// 마지막 브랜치의 커밋이 이미 게시됐고, 묶음 끝이 그 커밋을 둘째 부모로 가진 병합이다. 그래서 ship.sh는 바꾸지 않았다.
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
  if ! git push --quiet origin "$target"; then
    git reset --hard --quiet "$base"
    echo "push was rejected"
    exit 20
  fi
fi
exec bash scripts/gate/ship.sh "$5" "$6"
"#;

/// 코디 브랜치를 묶음 끝으로 빨리 감는다. 시작 폴더는 코디 워크트리다. 인자: 대상, 묶음 끝.
const ADVANCE: &str = r#"set -u
[ "$(git symbolic-ref --quiet --short HEAD)" = "$1" ] || { echo "the coordinator worktree is not on $1"; exit 1; }
[ -z "$(git status --porcelain)" ] || exit 21
git merge --ff-only --quiet "$2" || exit 20
"#;

/// 데몬 작업 하나. 시작할 때 끝나지 않은 묶음을 거두고, 그 뒤 큐가 빌 때까지 묶음을 돌린다.
pub(super) async fn run(app: Arc<App>) {
    recover(&app, CHECK_LOCK).await;
    loop {
        loop {
            let next = match app.graph.queued_targets() {
                Ok(targets) => targets.into_iter().next(),
                Err(e) => {
                    eprintln!("loom: merge queue: {e}");
                    None
                }
            };
            let Some((repo, target)) = next else { break };
            if let Err(e) = batch(&app, repo, &target, CHECK_LOCK).await {
                eprintln!("loom: merge queue {target}: {e}");
                break;
            }
        }
        tokio::select! {
            () = app.merge_queue.notified() => {}
            () = tokio::time::sleep(TICK) => {}
        }
    }
}

/// 한 대상의 큐 전체를 한 묶음으로 끝낸다. 끝나면 묶음의 모든 브랜치가 병합·반려·실패했거나 큐에 다시 섰다.
async fn batch(app: &App, repo: RepoId, target: &str, lock: &str) -> Result<(), Refusal> {
    let queue = app.graph.queue(repo, target)?;
    let branches: Vec<&str> = queue.iter().map(|q| q.branch.as_str()).collect();
    let id = app.graph.batch_open(repo, target, &branches)?;
    let run = Batch {
        app,
        id,
        repo,
        dir: repo_path(app, repo)?,
        target: target.to_owned(),
        lock,
    };
    if let Err(e) = run.run(queue.clone()).await {
        run.fail_rest(&queue, &e.to_string());
    }
    Ok(app.graph.batch_close(id)?)
}

/// 데몬이 묶음 중에 끝났다: 검사 전이면 모두 큐에 다시 세운다. 검사를 지났으면 게시를 잇는다(배포가 사람의 데몬을 다시 띄우면 이 길이다).
async fn recover(app: &App, lock: &str) {
    let stale = match app.graph.stale_batches() {
        Ok(stale) => stale,
        Err(e) => return eprintln!("loom: merge queue recovery: {e}"),
    };
    for stale in stale {
        let Ok(dir) = repo_path(app, stale.repo) else {
            continue;
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
                run.publish(&stale.members, &base, &tip, 0).await
            }
            _ => {
                stale.members.iter().for_each(|m| run.requeue(m));
                Ok(())
            }
        };
        if let Err(e) = done {
            run.fail_rest(&stale.members, &e.to_string());
        }
        if let Err(e) = app.graph.batch_close(stale.id) {
            eprintln!("loom: merge queue recovery: {e}");
        }
    }
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
    /// 원래 체크아웃(데몬이 보는 경로).
    dir: PathBuf,
    target: String,
    lock: &'a str,
}

/// 대상 브랜치의 자리.
enum Dest {
    /// 원래 체크아웃의 브랜치: 원격에 맞추고, 게시하고, 배포를 확인한다.
    Base,
    /// 코디 워크트리의 브랜치: 빨리 감기만 한다.
    Coordinator { worktree: String },
}

/// 쌓기·검사 한 번의 결과.
enum Trial {
    Conflict { index: usize, log: Option<String> },
    Failed { log: Option<String> },
    Passed { base: String, tip: String },
}

impl Batch<'_> {
    async fn run(&self, queue: Vec<Queued>) -> Result<(), Refusal> {
        let mut members = Vec::new();
        for member in queue {
            let findings = self.audit(&member).await?;
            match protocol::audit_end(&findings) {
                Some(end) => self.settle(
                    &member,
                    Ended {
                        end,
                        merged: None,
                        cause: None,
                        log: None,
                        lines: self.lines(),
                        findings,
                    },
                ),
                None => members.push(member),
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
        self.bisect(members, &into).await
    }

    /// 묶음 안 이분 탐색. `prefix`는 대상 위에서 검사를 지난 앞쪽, `suspects`는 그 뒤에 쌓으면 실패를 담은 쪽이다(`known_bad`일 때).
    /// 실패를 더하는 첫 브랜치 하나를 찾아 반려하고 나머지는 큐에 다시 세운다. 검사 수는 처음 하나와 많아야 ceil(log2 n)이다.
    /// 여러 브랜치가 함께 있어야 깨지면 그 가운데 뒤에 쌓인 브랜치가 반려된다. 흔들리는 검사 실패도 한 브랜치를 반려한다: 담당이 다시 `loom ready`한다.
    async fn bisect(&self, members: Vec<Queued>, into: &Dest) -> Result<(), Refusal> {
        let mut prefix: Vec<Queued> = Vec::new();
        let mut suspects = members;
        let mut known_bad = false;
        // 범인에게 보낼 기록: 그 브랜치가 든 마지막 실패 검사.
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
            match self.trial(&trial, step).await? {
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
                Trial::Passed { base, tip } if half == suspects.len() => {
                    // 거두기가 게시를 잇는 값이다. 묶음 전체가 지난 검사만 남긴다.
                    self.app.graph.batch_checked(self.id, &base, &tip)?;
                    return self.publish_into(&trial, &base, &tip, step, into).await;
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

    /// 쌓고 검사한다. 흔적 한 줄을 남긴다.
    async fn trial(&self, trial: &[Queued], step: u32) -> Result<Trial, Refusal> {
        let mut args = vec![
            format!("batch-{}-{step}.log", self.id.0),
            self.target.clone(),
            self.lock.to_owned(),
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
            (0, _, Some((tip, base))) => (
                Trial::Passed {
                    base: base.clone(),
                    tip: tip.clone(),
                },
                BatchResult::Passed,
            ),
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
                    0 => self.merged(members, tip, None).await,
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

    /// 대상의 자리를 git에서 다시 읽고 게시한다. 거두기(`recover`)가 쓴다.
    async fn publish(
        &self,
        members: &[Queued],
        base: &str,
        tip: &str,
        step: u32,
    ) -> Result<(), Refusal> {
        let into = self.into().await?;
        self.publish_into(members, base, tip, step, &into).await
    }

    /// 원래 체크아웃에 게시하고 배포를 확인한다.
    async fn publish_base(
        &self,
        members: &[Queued],
        base: &str,
        tip: &str,
        step: u32,
    ) -> Result<(), Refusal> {
        let last = members.last().expect("a published batch has members");
        let args = [
            format!("batch-{}-publish.log", self.id.0),
            self.target.clone(),
            base.to_owned(),
            tip.to_owned(),
            last.branch.clone(),
            last.head.clone(),
        ];
        let (code, out) = script(&self.dir, PUBLISH, &args).await?;
        let log = Lines::read(&out).log;
        match code {
            0 => self.merged(members, tip, Some("deploy: confirmed")).await,
            SHIP_DEPLOY_FAILED => {
                self.merged(members, tip, Some("deploy: failed (see the log)"))
                    .await
            }
            MOVED => {
                self.moved(members, step);
                Ok(())
            }
            _ => {
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
        tip: &str,
        deploy: Option<&str>,
    ) -> Result<(), Refusal> {
        let base = base_branch(&self.dir).await? == self.target;
        for m in members {
            let Some(sha) = merge_commit(&self.dir, &m.head, tip).await? else {
                self.settle(
                    m,
                    self.failed(
                        format!("no merge commit of {} is on the batch tip {tip}", m.head),
                        None,
                    ),
                );
                continue;
            };
            let mut findings = Vec::new();
            if base {
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
            lines.extend(deploy.map(str::to_owned));
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
        // 지운 브랜치는 검토한 커밋으로 병합한다. 움직인 브랜치만 막는다.
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
        members.iter().for_each(|m| self.requeue(m));
    }

    /// 아직 `Merging`인 브랜치를 실패로 끝낸다. 묶음이 멈추지 않게 한다(스크립트를 띄우지 못함, git 오류).
    fn fail_rest(&self, members: &[Queued], cause: &str) {
        for m in members {
            let ended = self.failed(format!("the merge queue stopped: {cause}"), None);
            // 이미 끝낸 브랜치는 전이가 거절된다. 그대로 둔다.
            let _ = self.app.graph.merge_end(self.repo, &m.branch, ended);
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
        vec![format!("batch: {}", self.id.0)]
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
        Ended {
            end: MergeEnd::Failed,
            merged: None,
            cause: Some(cause),
            log,
            lines: self.lines(),
            findings: Vec::new(),
        }
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

/// 스크립트 표준 출력의 결과 줄.
#[derive(Default, Debug, PartialEq)]
struct Lines {
    log: Option<String>,
    conflict: Option<usize>,
    check: Option<u64>,
    /// (묶음 끝, 쌓은 대상 커밋).
    tip: Option<(String, String)>,
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
                (Some("tip"), Some(tip), Some(base)) => {
                    lines.tip = Some((tip.to_owned(), base.to_owned()))
                }
                _ => {}
            }
        }
        lines
    }
}

/// `dir`에서 bash 스크립트 글 하나를 띄우고 끝 코드와 표준 출력을 돌려준다. 신호로 끝나면 -1이다.
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
