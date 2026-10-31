use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::distro::Ran;
use crate::git;
use crate::place::{self, Files, Machine};
use crate::protocol::Refusal;

pub const CONFLICT: i32 = 10;
pub const CHECK_FAILED: i32 = 14;
pub const MOVED: i32 = 20;
pub const DIRTY: i32 = 21;
const BATCH_WAIT: Duration = Duration::from_secs(60 * 60);

pub const CLEAN: &str = r#"common=$(git rev-parse --path-format=absolute --git-common-dir)
for wt in "$common"/loom-batches/*; do
  [ -e "$wt" ] || continue
  git worktree remove --force "$wt" 2>/dev/null
  rm -rf "$wt"
done
git worktree prune
"#;

pub const SYNC: &str = r#"set -u
target=$1
[ "$(git symbolic-ref --quiet --short HEAD)" = "$target" ] || { echo "the main checkout is not on $target"; exit 1; }
[ -z "$(git status --porcelain --untracked-files=all)" ] || { echo "the main checkout has uncommitted changes"; exit 1; }
git fetch --quiet origin "$target" || { echo "git fetch origin $target failed"; exit 1; }
git merge --ff-only --quiet "origin/$target" || { echo "$target and origin/$target diverged"; exit 1; }
[ "$(git rev-parse HEAD)" = "$(git rev-parse "origin/$target")" ] || { echo "$target has commits that are not on origin/$target"; exit 1; }
"#;

pub const STACK: &str = r#"set -u
common=$(git rev-parse --path-format=absolute --git-common-dir)
wt=$common/loom-batches/$1 log=$common/loom-logs/$1.log lock=$2 base=$3 mark=$4
shift 4
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
    git -C "$wt" merge --no-ff -m "Merge branch '$1'$mark" "$2" || { git -C "$wt" merge --abort; echo "conflict $i" >&3; exit 10; }
  fi
  i=$((i + 1))
  shift 2
done
tip=$(git -C "$wt" rev-parse HEAD)
if [ -f "$wt/scripts/gate/check.sh" ]; then
  exec 9>>"$lock"
  if command -v flock >/dev/null 2>&1; then
    flock 9
  elif command -v perl >/dev/null 2>&1; then
    perl -MFcntl=:flock -e 'open(my $fd, ">&=", 9) or die "$!\n"; flock($fd, LOCK_EX) or die "$!\n"' || exit 1
  else
    echo "neither flock nor perl is on this machine; loom cannot lock $lock" >&3
    exit 1
  fi
  start=$(date +%s)
  (cd "$wt" && CARGO_TARGET_DIR="$common/loom-batch-target" bash scripts/gate/check.sh 9>&-)
  code=$?
  echo "check $(($(date +%s) - start))" >&3
  unused_min=2880
  find "$common/loom-batch-target" -type d -name .fingerprint | while read -r fingerprints; do
    profile=${fingerprints%/.fingerprint}
    find "$fingerprints" -mindepth 1 -maxdepth 1 -type d | while read -r unit; do
      [ -z "$(find "$unit" -mindepth 1 \( -amin -$unused_min -o -mmin -$unused_min \) -print -quit)" ] || continue
      rm -rf "$unit" "$profile/build/${unit##*/}" "$profile"/deps/*-"${unit##*-}"*
    done
    [ ! -d "$profile/incremental" ] || find "$profile/incremental" -mindepth 1 -maxdepth 1 -type d -mmin +$unused_min -exec rm -rf {} +
  done
  find "$common/loom-batch-target" -mindepth 1 -type d -empty -delete
  [ "$code" -eq 0 ] || exit 14
else
  echo "== no scripts/gate/check.sh"
fi
echo "tip $tip" >&3
"#;

pub const PUBLISH: &str = r#"set -u
common=$(git rev-parse --path-format=absolute --git-common-dir)
log=$common/loom-logs/$1 target=$2 base=$3 tip=$4
mkdir -p "$common/loom-logs"
exec 3>&1 >>"$log" 2>&1
echo "log $log" >&3
[ "$(git symbolic-ref --quiet --short HEAD)" = "$target" ] || { echo "the main checkout is not on $target"; exit 1; }
[ -z "$(git status --porcelain --untracked-files=all)" ] || { echo "the main checkout has uncommitted changes"; exit 1; }
git fetch --quiet origin "$target" || { echo "git fetch failed"; exit 1; }
if ! git merge-base --is-ancestor "$tip" "origin/$target"; then
  [ "$(git rev-parse "origin/$target")" = "$base" ] || { echo "origin/$target moved"; exit 20; }
  head=$(git rev-parse HEAD)
  [ "$head" = "$base" ] || [ "$head" = "$tip" ] || { echo "$target moved"; exit 20; }
  git merge --ff-only --quiet "$tip" || exit 1
  [ "$(git rev-parse HEAD)" = "$tip" ] || { echo "$target moved after the fast-forward to $tip"; exit 1; }
  if ! git push --quiet origin "$tip:refs/heads/$target"; then
    [ -z "$(git status --porcelain --untracked-files=all)" ] ||
      { echo "git push origin $target failed and the main checkout has uncommitted changes; loom kept them and did not undo the fast-forward to $tip"; exit 1; }
    git update-ref "refs/heads/$target" "$base" "$tip" ||
      { echo "$target moved during git push; loom kept it and did not undo the fast-forward to $tip"; exit 1; }
    git reset --hard --quiet
    git fetch --quiet origin "$target" || { echo "git fetch failed after git push failed"; exit 1; }
    [ "$(git rev-parse "origin/$target")" = "$base" ] || { echo "origin/$target moved during git push"; exit 20; }
    echo "git push origin $target failed"
    exit 1
  fi
fi
"#;

pub const SHIP: &str = r#"set -u
common=$(git rev-parse --path-format=absolute --git-common-dir)
log=$common/loom-logs/$1
mkdir -p "$common/loom-logs"
exec 3>&1 >>"$log" 2>&1
echo "log $log" >&3
[ -f scripts/gate/ship.sh ] || { echo "no scripts/gate/ship.sh"; exit 0; }
exec bash scripts/gate/ship.sh "$2" "$3"
"#;

pub const UNDO: &str = r#"set -u
target=$1 base=$2 tip=$3
git merge-base --is-ancestor "$tip" "refs/heads/$target" || exit 0
[ "$(git symbolic-ref --quiet --short HEAD)" = "$target" ] || { echo "the main checkout is not on $target"; exit 1; }
[ -z "$(git status --porcelain --untracked-files=all)" ] || { echo "the main checkout has uncommitted changes"; exit 1; }
git update-ref "refs/heads/$target" "$base" "$tip" || { echo "$target moved past the batch tip"; exit 1; }
git reset --hard --quiet
"#;

pub const ADVANCE: &str = r#"set -u
[ "$(git symbolic-ref --quiet --short HEAD)" = "$1" ] || { echo "the coordinator worktree is not on $1"; exit 1; }
[ -z "$(git status --porcelain --untracked-files=all)" ] || exit 21
git merge --ff-only --quiet "$2" || exit 20
"#;

pub async fn merge_commit(
    files: Files<'_>,
    repo: &Path,
    head: &str,
    tip: &str,
) -> Result<Option<String>, Refusal> {
    let listed = git::run(
        files,
        repo,
        &["log", "--first-parent", "--format=%H %P", tip],
    )
    .await
    .map_err(Refusal::Git)?;
    Ok(listed.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let sha = parts.next()?;
        parts.skip(1).any(|p| p == head).then(|| sha.to_owned())
    }))
}

pub async fn base_branch(files: Files<'_>, repo: &Path) -> Result<String, Refusal> {
    super::listed(files, repo)
        .await
        .map_err(Refusal::Git)?
        .into_iter()
        .find(|w| w.original)
        .and_then(|w| w.branch)
        .ok_or_else(|| Refusal::Git("the main checkout is not on a branch".into()))
}

pub async fn contains(
    files: Files<'_>,
    repo: &Path,
    older: &str,
    newer: &str,
) -> Result<bool, Refusal> {
    git::holds(files, repo, &["merge-base", "--is-ancestor", older, newer])
        .await
        .map_err(Refusal::Git)
}

pub async fn target_base(files: Files<'_>, repo: &Path, target: &str) -> Result<String, Refusal> {
    git::run(
        files,
        repo,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/heads/{target}^{{commit}}"),
        ],
    )
    .await
    .map_err(Refusal::Git)
}

pub async fn fetch(files: Files<'_>, repo: &Path, target: &str) -> Result<String, Refusal> {
    git::run(files, repo, &["fetch", "--quiet", "origin", target])
        .await
        .map_err(Refusal::Git)
}

pub async fn published(files: Files<'_>, repo: &Path, tip: &str, done: &str) -> bool {
    git::run(files, repo, &["merge-base", "--is-ancestor", tip, done])
        .await
        .is_ok()
}

pub async fn first_parents(files: Files<'_>, repo: &Path, sha: &str) -> Result<String, Refusal> {
    git::run(
        files,
        repo,
        &["rev-list", "--first-parent", &format!("{sha}^1")],
    )
    .await
    .map_err(Refusal::Git)
}

pub async fn branch_head(files: Files<'_>, repo: &Path, branch: &str) -> Result<String, String> {
    git::run(
        files,
        repo,
        &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
    )
    .await
}

pub async fn clean_at(files: Files<'_>, source: &Path, head: &str) -> bool {
    let now = git::run(files, source, &["rev-parse", "HEAD"]).await;
    let changes = git::run(
        files,
        source,
        &["status", "--porcelain", "--untracked-files=no"],
    )
    .await;
    matches!((now, changes), (Ok(now), Ok(changes)) if now.trim() == head && changes.trim().is_empty())
}

pub async fn bringing_merge(
    files: Files<'_>,
    dir: &Path,
    head: &str,
    base: &str,
    tip: &str,
) -> Result<Option<String>, Refusal> {
    let listed = git::run(
        files,
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
        if contains(files, dir, head, sha).await? {
            return Ok(Some(sha.to_owned()));
        }
    }
    Ok(None)
}

#[derive(Default, Debug, PartialEq)]
pub struct Lines {
    pub log: Option<String>,
    pub conflict: Option<usize>,
    pub check: Option<u64>,
    pub tip: Option<String>,
}

impl Lines {
    pub fn read(out: &str) -> Lines {
        let mut lines = Lines::default();
        for line in out.lines() {
            let mut words = line.split_whitespace();
            match (words.next(), words.next(), words.next()) {
                (Some("log"), _, _) => lines.log = line.strip_prefix("log ").map(str::to_owned),
                (Some("conflict"), Some(i), _) => lines.conflict = i.parse().ok(),
                (Some("check"), Some(secs), _) => lines.check = secs.parse().ok(),
                (Some("tip"), Some(tip), _) => lines.tip = Some(tip.to_owned()),
                _ => {}
            }
        }
        lines
    }
}

pub async fn script(
    files: Files<'_>,
    dir: &Path,
    text: &str,
    args: &[String],
) -> Result<(i32, String), Refusal> {
    let out = match place::of(dir) {
        Machine::Pc | Machine::Wsl(_) => {
            let mut cmd = crate::wsl::command(&dir.to_string_lossy(), "bash");
            cmd.arg("-c").arg(text).arg("loom-queue").args(args);
            cmd.stdin(std::process::Stdio::null());
            let out = tokio::process::Command::from(cmd)
                .output()
                .await
                .map_err(|e| Refusal::Git(format!("could not run bash: {e}")))?;
            Ran {
                code: out.status.code(),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            }
        }
        Machine::Ssh(at) => {
            let enter = [crate::ssh::LOGIN_PATH, r#"cd "$1" && shift && exec "$@""#].concat();
            let argv = [
                "sh",
                "-c",
                &enter,
                "sh",
                &at.path,
                "bash",
                "-c",
                text,
                "loom-queue",
            ];
            let argv = argv.into_iter().chain(args.iter().map(String::as_str));
            files
                .run(&at.host, git::remote(argv, &[]), BATCH_WAIT)
                .await
                .map_err(|e| Refusal::Git(format!("could not run bash on {}: {e}", at.host)))?
        }
    };
    let mut text = out.stdout;
    if out.code != Some(0) {
        text.push_str(&out.stderr);
    }
    Ok((out.code.unwrap_or(-1), text))
}

pub async fn still_merged(
    files: Files<'_>,
    dir: &Path,
    own: Option<Option<&str>>,
    branch: &str,
    target: &str,
) -> Result<bool, String> {
    if own.is_some_and(|on| on != Some(branch)) {
        return Ok(false);
    }
    let local = format!("refs/heads/{branch}");
    if git::run(files, dir, &["rev-parse", "--verify", "--quiet", &local])
        .await
        .is_err()
    {
        return Ok(true);
    }
    contains(files, dir, &local, &format!("refs/heads/{target}"))
        .await
        .map_err(|e| e.to_string())
}

pub async fn delete_merged(
    files: Files<'_>,
    dir: &Path,
    branch: &str,
    target: &str,
) -> Result<Option<String>, String> {
    let Some(at) = deletable_at(files, dir, branch, target).await else {
        return Ok(None);
    };
    let remote = format!("refs/remotes/origin/{branch}");
    let pushed = git::run(files, dir, &["rev-parse", "--verify", "--quiet", &remote])
        .await
        .ok();
    let mut kept = None;
    if let Some(seen) = pushed
        && contains(files, dir, &remote, &format!("refs/heads/{target}"))
            .await
            .map_err(|e| e.to_string())?
    {
        let lease = format!("--force-with-lease=refs/heads/{branch}:{}", seen.trim());
        kept = git::run(
            files,
            dir,
            &[
                "push",
                "--quiet",
                &lease,
                "origin",
                &format!(":refs/heads/{branch}"),
            ],
        )
        .await
        .err();
    }
    git::run(files, &at, &["branch", "-d", "--", branch]).await?;
    Ok(kept)
}

pub async fn delete_local_merged(
    files: Files<'_>,
    dir: &Path,
    branch: &str,
    target: &str,
) -> Result<(), String> {
    if let Some(at) = deletable_at(files, dir, branch, target).await {
        git::run(files, &at, &["branch", "-d", "--", branch]).await?;
    }
    Ok(())
}

async fn deletable_at(files: Files<'_>, dir: &Path, branch: &str, target: &str) -> Option<PathBuf> {
    let listed = super::worktrees(files, dir).await;
    let local = format!("refs/heads/{branch}");
    if listed.iter().any(|w| w.branch.as_deref() == Some(branch))
        || git::run(files, dir, &["rev-parse", "--verify", "--quiet", &local])
            .await
            .is_err()
    {
        return None;
    }
    Some(
        listed
            .iter()
            .find(|w| w.branch.as_deref() == Some(target))
            .map_or_else(|| dir.to_path_buf(), |w| PathBuf::from(&w.path)),
    )
}
