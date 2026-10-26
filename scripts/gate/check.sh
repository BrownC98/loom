#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

CARGO_STEP_MIB=5120
WINDOWS_TARGET_DIR=${CARGO_TARGET_DIR:-target}/windows-gate
capacity=$HOME/.agents/skills/dev-toolbox/scripts/capacity.py
headroom() { python3 "$capacity" claude 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["headroomMiB"])' 2>/dev/null; }
room() {
  [ -f "$capacity" ] || return 0
  local waited=0
  until [ "$(headroom)" -ge "$CARGO_STEP_MIB" ] 2>/dev/null; do
    [ "$waited" -ge 2400 ] && { echo "no memory headroom for 40 min"; return 1; }
    sleep 60
    waited=$((waited + 60))
  done
}
step() { echo "== $*"; "$@"; }

linux() {
  room; step memcap -m "${CARGO_STEP_MIB}M" cargo clippy --all-targets
  room; step memcap -m "${CARGO_STEP_MIB}M" cargo test --bin loom --test comments --test layers
  echo "== web/src/gen equals the commit"
  [ -z "$(git status --porcelain -- web/src/gen)" ] || { git status --short -- web/src/gen; return 1; }
  step memcap -m "${CARGO_STEP_MIB}M" cargo test --test comments -- --ignored --exact asked_questions_were_answered
}
windows() {
  export CARGO_TARGET_DIR=$WINDOWS_TARGET_DIR
  room; step memcap -m "${CARGO_STEP_MIB}M" cargo clippy --all-targets --target x86_64-pc-windows-gnu
}

[ -d web/node_modules ] && [ -d web/dist ] || step timeout 600 bash scripts/worktree/setup.sh
step cargo fmt --check
free=$(headroom) || free=
if [ "$free" -ge $((2 * CARGO_STEP_MIB)) ] 2>/dev/null; then
  echo "== linux and windows cargo groups in parallel (headroom $free MiB)"
  logs=$(mktemp -d)
  trap 'rm -rf "$logs"' EXIT
  (linux) >"$logs/linux" 2>&1 &
  linux_pid=$!
  (windows) >"$logs/windows" 2>&1 &
  windows_pid=$!
  failed=
  wait "$linux_pid" || failed="$failed linux"
  wait "$windows_pid" || failed="$failed windows"
  for group in linux windows; do
    echo "== $group group output"
    cat "$logs/$group"
  done
  [ -z "$failed" ] || { echo "== cargo group failed:$failed"; exit 1; }
else
  echo "== linux and windows cargo groups in series (headroom ${free:-unknown} MiB)"
  (linux)
  (windows)
fi
step npm --prefix web run check
step npm --prefix web run build
echo "== all checks passed"
