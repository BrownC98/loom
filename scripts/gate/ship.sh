#!/usr/bin/env bash
set -uo pipefail
merged=$2
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
base=$(git symbolic-ref --quiet --short HEAD) || { echo "the main checkout is not on a branch"; exit 1; }
published=origin/$base
git fetch --quiet origin "$base" || { echo "git fetch failed; the merge queue already published, so tell the root to check the deploy"; exit 12; }
git merge-base --is-ancestor "$merged" "$published" || { echo "$merged is not in $published; the merge queue publishes before ship.sh, so tell the root"; exit 12; }

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
