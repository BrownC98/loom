#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

capacity=$HOME/.agents/skills/dev-toolbox/scripts/capacity.py
room() {
  [ -f "$capacity" ] || return 0
  local waited=0
  until [ "$(python3 "$capacity" claude 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["headroomMiB"] >= 8192)' 2>/dev/null)" = True ]; do
    [ "$waited" -ge 2400 ] && { echo "no memory headroom for 40 min"; return 1; }
    sleep 60
    waited=$((waited + 60))
  done
}
step() { echo "== $*"; "$@"; }

[ -d web/node_modules ] && [ -d web/dist ] || step timeout 600 bash scripts/worktree/setup.sh
step cargo fmt --check
room; step memcap -m 8G cargo clippy --all-targets
room; step memcap -m 8G cargo test
room; step memcap -m 8G cargo clippy --all-targets --target x86_64-pc-windows-gnu
room; step memcap -m 8G cargo test --target x86_64-pc-windows-gnu
step npm --prefix web run check
step npm --prefix web test
step npm --prefix web run build
echo "== all checks passed"
