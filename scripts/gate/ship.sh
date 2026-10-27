#!/usr/bin/env bash
set -uo pipefail
merged=$2
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

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
