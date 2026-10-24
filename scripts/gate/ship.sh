#!/usr/bin/env bash
set -uo pipefail
sha=$2
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
base=$(git symbolic-ref --quiet --short HEAD) || { echo "the main checkout is not on a branch"; exit 1; }
published=origin/$base
git fetch --quiet origin "$base" || { echo "git fetch failed; the merge queue already published, so tell the root to check the deploy"; exit 12; }
git merge-base --is-ancestor "$sha" "$published" || { echo "$sha is not in $published; the merge queue publishes before ship.sh, so tell the root"; exit 12; }
merged=$(git log --first-parent --format='%H %P' "$published" | awk -v h="$sha" '{ for (i = 3; i <= NF; i++) if ($i == h) { print $1; exit } }')
[ -n "$merged" ] || { echo "$sha is in $published without a gate merge commit; tell the root"; exit 12; }

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
