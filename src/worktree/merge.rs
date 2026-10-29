use std::path::{Path, PathBuf};

use crate::git;
use crate::protocol::Refusal;

pub const CONFLICT: i32 = 10;
pub const CHECK_FAILED: i32 = 14;
pub const MOVED: i32 = 20;
pub const DIRTY: i32 = 21;

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
  flock 9
  start=$(date +%s)
  (cd "$wt" && CARGO_TARGET_DIR="$common/loom-batch-target" bash scripts/gate/check.sh 9>&-)
  code=$?
  echo "check $(($(date +%s) - start))" >&3
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

pub async fn merge_commit(repo: &Path, head: &str, tip: &str) -> Result<Option<String>, Refusal> {
    let listed = git::run(repo, &["log", "--first-parent", "--format=%H %P", tip])
        .await
        .map_err(Refusal::Git)?;
    Ok(listed.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let sha = parts.next()?;
        parts.skip(1).any(|p| p == head).then(|| sha.to_owned())
    }))
}

pub async fn base_branch(repo: &Path) -> Result<String, Refusal> {
    super::listed(repo)
        .await
        .map_err(Refusal::Git)?
        .into_iter()
        .find(|w| w.original)
        .and_then(|w| w.branch)
        .ok_or_else(|| Refusal::Git("the main checkout is not on a branch".into()))
}

pub async fn contains(repo: &Path, older: &str, newer: &str) -> Result<bool, Refusal> {
    git::holds(repo, &["merge-base", "--is-ancestor", older, newer])
        .await
        .map_err(Refusal::Git)
}

pub async fn target_base(repo: &Path, target: &str) -> Result<String, Refusal> {
    git::run(
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

pub async fn fetch(repo: &Path, target: &str) -> Result<String, Refusal> {
    git::run(repo, &["fetch", "--quiet", "origin", target])
        .await
        .map_err(Refusal::Git)
}

pub async fn published(repo: &Path, tip: &str, done: &str) -> bool {
    git::run(repo, &["merge-base", "--is-ancestor", tip, done])
        .await
        .is_ok()
}

pub async fn first_parents(repo: &Path, sha: &str) -> Result<String, Refusal> {
    git::run(repo, &["rev-list", "--first-parent", &format!("{sha}^1")])
        .await
        .map_err(Refusal::Git)
}

pub async fn branch_head(repo: &Path, branch: &str) -> Result<String, String> {
    git::run(
        repo,
        &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
    )
    .await
}

pub async fn clean_at(source: &Path, head: &str) -> bool {
    let now = git::run(source, &["rev-parse", "HEAD"]).await;
    let changes = git::run(source, &["status", "--porcelain", "--untracked-files=no"]).await;
    matches!((now, changes), (Ok(now), Ok(changes)) if now.trim() == head && changes.trim().is_empty())
}

pub async fn bringing_merge(
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
        if contains(dir, head, sha).await? {
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

pub async fn script(dir: &Path, text: &str, args: &[String]) -> Result<(i32, String), Refusal> {
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

pub async fn still_merged(
    dir: &Path,
    own: Option<Option<&str>>,
    branch: &str,
    target: &str,
) -> Result<bool, String> {
    if own.is_some_and(|on| on != Some(branch)) {
        return Ok(false);
    }
    let local = format!("refs/heads/{branch}");
    if git::run(dir, &["rev-parse", "--verify", "--quiet", &local])
        .await
        .is_err()
    {
        return Ok(true);
    }
    contains(dir, &local, &format!("refs/heads/{target}"))
        .await
        .map_err(|e| e.to_string())
}

pub async fn delete_merged(
    dir: &Path,
    branch: &str,
    target: &str,
) -> Result<Option<String>, String> {
    let Some(at) = deletable_at(dir, branch, target).await else {
        return Ok(None);
    };
    let remote = format!("refs/remotes/origin/{branch}");
    let pushed = git::run(dir, &["rev-parse", "--verify", "--quiet", &remote])
        .await
        .ok();
    let mut kept = None;
    if let Some(seen) = pushed
        && contains(dir, &remote, &format!("refs/heads/{target}"))
            .await
            .map_err(|e| e.to_string())?
    {
        let lease = format!("--force-with-lease=refs/heads/{branch}:{}", seen.trim());
        kept = git::run(
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
    git::run(&at, &["branch", "-d", "--", branch]).await?;
    Ok(kept)
}

pub async fn delete_local_merged(dir: &Path, branch: &str, target: &str) -> Result<(), String> {
    if let Some(at) = deletable_at(dir, branch, target).await {
        git::run(&at, &["branch", "-d", "--", branch]).await?;
    }
    Ok(())
}

async fn deletable_at(dir: &Path, branch: &str, target: &str) -> Option<PathBuf> {
    let listed = super::worktrees(dir).await;
    let local = format!("refs/heads/{branch}");
    if listed.iter().any(|w| w.branch.as_deref() == Some(branch))
        || git::run(dir, &["rev-parse", "--verify", "--quiet", &local])
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
