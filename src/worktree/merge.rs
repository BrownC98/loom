use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::distro::Ran;
use crate::git;
use crate::place::{self, Files, Machine};
use api::graph::errors::Refusal;

pub const CONFLICT: i32 = 10;
pub const CHECK_FAILED: i32 = 14;
pub const MOVED: i32 = 20;
pub const DIRTY: i32 = 21;
const BATCH_WAIT: Duration = Duration::from_secs(60 * 60);
macro_rules! batches {
    () => {
        "loom-batches"
    };
}

pub const BATCHES: &str = batches!();

macro_rules! common {
    () => {
        concat!(
            "common=$(git rev-parse --path-format=absolute --git-common-dir)\nbatches=$common/",
            batches!(),
            "\n"
        )
    };
}

pub const CLEAN: &str = concat!(
    common!(),
    r#"for wt in "$batches"/*; do
  [ -e "$wt" ] || continue
  [ "$wt" = "$batches/check" ] && continue
  git worktree remove --force "$wt" 2>/dev/null
  rm -rf "$wt"
done
git worktree prune
"#
);

pub const SYNC: &str = r#"set -u
target=$1
[ "$(git symbolic-ref --quiet --short HEAD)" = "$target" ] || { echo "the main checkout is not on $target"; exit 1; }
[ -z "$(git status --porcelain --untracked-files=all)" ] || { echo "the main checkout has uncommitted changes"; exit 1; }
git fetch --quiet origin "$target" || { echo "git fetch origin $target failed"; exit 1; }
git merge --ff-only --quiet "origin/$target" || { echo "$target and origin/$target diverged"; exit 1; }
[ "$(git rev-parse HEAD)" = "$(git rev-parse "origin/$target")" ] || { echo "$target has commits that are not on origin/$target"; exit 1; }
"#;

macro_rules! open {
    () => {
        concat!(
            "set -u\n",
            common!(),
            r#"wt=$batches/check log=$common/loom-logs/$1.log lock=$common/loom-check.lock base=$2 mark=$3
trees=$common/loom-logs/passed-trees
mkdir -p "$common/loom-logs" "$batches"
exec 3>&1 >>"$log" 2>&1
echo "log $log" >&3
"#
        )
    };
}

macro_rules! stack_and_check {
    () => {
        r#"shift 3
exec 9>>"$lock"
if command -v flock >/dev/null 2>&1; then
  flock 9
elif command -v perl >/dev/null 2>&1; then
  perl -MFcntl=:flock -e 'open(my $fd, ">&=", 9) or die "$!\n"; flock($fd, LOCK_EX) or die "$!\n"' || exit 1
else
  echo "neither flock nor perl is on this machine; loom cannot lock $lock" >&3
  exit 1
fi
rm -rf "$common/loom-batch-target" "${XDG_CACHE_HOME:-$HOME/.cache}/loom-batch-target"
if ! { [ "$(git -C "$wt" rev-parse --is-inside-work-tree 2>/dev/null)" = true ] &&
  git -C "$wt" reset --hard --quiet &&
  git -C "$wt" checkout --detach --force --quiet "$base" &&
  git -C "$wt" clean -fdq; }; then
  rm -rf "$wt"
  git worktree prune
  git worktree add --detach "$wt" "$base" || exit 1
fi
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
key=$({ git -C "$wt" rev-parse 'HEAD^{tree}'; git -C "$wt" rev-list --no-merges HEAD | sort; } | git hash-object --stdin)
if [ ! -f "$wt/scripts/gate/check.sh" ]; then
  echo "== no scripts/gate/check.sh"
  pass
elif grep -qxF "$key" "$trees" 2>/dev/null; then
  echo "== the same files and commits passed scripts/gate/check.sh before (key $key); loom does not run it again"
  pass
else
  start=$(date +%s)
  (cd "$wt" && bash scripts/gate/check.sh) 9>&-
  code=$?
  if [ "$code" -eq 0 ]; then
    pass
    { tail -n 199 "$trees" 2>/dev/null; echo "$key"; } > "$trees.new" && mv "$trees.new" "$trees"
  fi
  echo "check $(($(date +%s) - start))" >&3
  cap_kib=$((32 * 1024 * 1024))
  if [ "$(du -sk "$wt" | cut -f1)" -gt "$cap_kib" ]; then
    echo "== $wt is over $((cap_kib / 1024 / 1024))GiB; loom deletes its ignored files"
    git -C "$wt" clean -fdxq
  fi
  [ "$code" -eq 0 ] || exit 14
fi
echo "tip $tip" >&3
"#
    };
}

pub const STACK: &str = concat!(
    open!(),
    r#"passed=$common/loom-logs/${1%-*}.passed
rm -f "$passed"
members=$(shift 3; printf 'member %s %s\n' "$@")
pass() { printf 'base %s\ntip %s\n%s\n' "$base" "$tip" "$members" > "$passed.new" && mv "$passed.new" "$passed"; }
"#,
    stack_and_check!()
);

pub const CHECK: &str = concat!(open!(), "pass() { :; }\n", stack_and_check!());

pub const PASSED: &str = r#"common=$(git rev-parse --path-format=absolute --git-common-dir)
cat "$common/loom-logs/batch-$1.passed" 2>/dev/null
exit 0
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

pub const TIDY_LOGS: &str = r#"set -u
common=$(git rev-parse --path-format=absolute --git-common-dir) || exit 1
mins=$1
shift
n=0 kib=0
for f in "$common/loom-logs"/batch-* "$common/loom-logs"/check-* "$common/loom-logs"/deploy-*; do
  [ -f "$f" ] || continue
  name=${f##*/}
  for open in "$@"; do
    case $name in "$open"-* | "$open".*) continue 2 ;; esac
  done
  old=$(find "$f" -mmin +"$mins" -print 2>/dev/null) || continue
  [ -n "$old" ] || continue
  k=$(du -sk "$f" | cut -f1)
  rm -f -- "$f" && n=$((n + 1)) kib=$((kib + ${k:-0}))
done
echo "$n $kib"
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
    pub base: Option<String>,
    pub members: Vec<(String, String)>,
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
                (Some("base"), Some(base), _) => lines.base = Some(base.to_owned()),
                (Some("member"), Some(branch), Some(head)) => {
                    lines.members.push((branch.to_owned(), head.to_owned()))
                }
                _ => {}
            }
        }
        lines
    }
}

#[derive(Default)]
pub struct CheckLocks(std::sync::Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>);

impl CheckLocks {
    pub async fn script(
        &self,
        files: Files<'_>,
        dir: &Path,
        text: &str,
        args: &[String],
    ) -> Result<(i32, String), Refusal> {
        let lock = self
            .0
            .lock()
            .unwrap()
            .entry(dir.to_path_buf())
            .or_default()
            .clone();
        let _held = lock.lock().await;
        script(files, dir, text, args).await
    }
}

pub async fn script(
    files: Files<'_>,
    dir: &Path,
    text: &str,
    args: &[String],
) -> Result<(i32, String), Refusal> {
    script_within(files, dir, text, args, BATCH_WAIT).await
}

pub async fn script_within(
    files: Files<'_>,
    dir: &Path,
    text: &str,
    args: &[String],
    limit: Duration,
) -> Result<(i32, String), Refusal> {
    let out = match place::of(dir) {
        Machine::Pc | Machine::Wsl(_) => {
            let mut cmd = crate::wsl::command(&dir.to_string_lossy(), "bash");
            cmd.arg("-c").arg(text).arg("loom-queue").args(args);
            let out = crate::timed::ended_within(tokio::process::Command::from(cmd), limit)
                .await
                .map_err(|e| Refusal::Git(format!("bash: {e}")))?;
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
                .run(&at.host, git::remote(argv, &[]), limit)
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
