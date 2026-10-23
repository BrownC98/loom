#!/usr/bin/env bash
set -uo pipefail
branch=$1
sha=$2
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
base=$(git symbolic-ref --quiet --short HEAD) || { echo "the main checkout is not on a branch"; exit 1; }
merge_of() { git log --first-parent --format='%H %P' "$1" | awk -v h="$sha" '{ for (i = 3; i <= NF; i++) if ($i == h) { print $1; exit } }'; }

if [ -n "$(git status --porcelain)" ]; then echo "the main checkout has uncommitted changes"; exit 1; fi
git fetch --quiet origin "$base" || { echo "git fetch failed; call loom merge again"; exit 12; }
published=origin/$base

if git merge-base --is-ancestor "$sha" "$published"; then
  merged=$(merge_of "$published")
  if [ -z "$merged" ]; then echo "$sha is already in $published without a gate merge commit (merged outside the gate); tell the root"; exit 12; fi
  echo "resume: $merged already merges $sha and is published"
  git merge --ff-only --quiet "$published" || { echo "$base and $published diverged"; exit 1; }
else
  if git merge-base --is-ancestor "$sha" HEAD; then
    local_merge=$(merge_of HEAD)
    unpublished=$(git rev-list --first-parent "$published..HEAD")
    if [ -z "$local_merge" ] || [ "$unpublished" != "$local_merge" ]; then
      echo "$base has unpublished commits besides the merge of $sha: $(echo $unpublished)"
      exit 1
    fi
    echo "resume: dropping the unpublished merge $local_merge and merging again on $published"
    git reset --hard --quiet "$published"
  fi
  git merge --ff-only --quiet "$published" || { echo "$base and $published diverged"; exit 1; }
  before=$(git rev-parse HEAD)
  if ! git merge-base --is-ancestor HEAD "$sha"; then
    echo "$base moved: merge $base into $branch again"
    exit 10
  fi
  if ! git merge --no-ff -m "Merge branch '$branch'" "$sha"; then
    git merge --abort
    exit 10
  fi
  merged=$(git rev-parse HEAD)
  if ! git push --quiet origin "$base"; then
    git reset --hard --quiet "$before"
    echo "push was rejected; call loom merge again"
    exit 12
  fi
fi

sleep 20
deployed() {
  local built=~/.loom/host-build
  git -C "$built" merge-base --is-ancestor "$merged" HEAD 2>/dev/null &&
    tail -1 ~/.loom/host.log | grep -q "^loom 0" &&
    [ ~/.loom/host.log -nt "$built/.git/HEAD" ]
}
for _ in $(seq 180); do
  if deployed; then
    tail -1 ~/.loom/host.log
    echo "$merged"
    exit 0
  fi
  sleep 5
done
tail -30 ~/.loom/host.log
echo "$merged"
exit 11
