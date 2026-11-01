#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

CLIPPY_SIZE=3G
TEST_SIZE=4608M
DESKTOP_SIZE=3584M
WINDOWS_TARGET_DIR=${CARGO_TARGET_DIR:-target}/windows-gate
step() { echo "== $*"; "$@"; }

linux() {
  step memcap -m "$CLIPPY_SIZE" cargo clippy --all-targets
  step memcap -m "$TEST_SIZE" cargo test --lib --bin loom --test comments --test layers --test contract
  echo "== web/src/gen equals the commit"
  [ -z "$(git status --porcelain -- web/src/gen)" ] || { git status --short -- web/src/gen; return 1; }
  step memcap -m "$TEST_SIZE" cargo test --test comments -- --ignored --exact asked_questions_were_answered
}
desktop() {
  cd desktop
  step cargo fmt --check
  step memcap -m "$DESKTOP_SIZE" cargo clippy --all-targets
  step memcap -m "$DESKTOP_SIZE" cargo test
}
windows() {
  export CARGO_TARGET_DIR=$WINDOWS_TARGET_DIR
  step memcap -m "$CLIPPY_SIZE" cargo clippy --all-targets --target x86_64-pc-windows-gnu
}

[ -d web/node_modules ] && [ -d web/dist ] || step timeout 600 bash scripts/worktree/setup.sh
step cargo fmt --check
echo "== merged DB change files stay as they are"
merged_changes=$(git diff --name-status --diff-filter=MDR origin/main...HEAD -- 'src/db/changes/*.sql')
[ -z "$merged_changes" ] || { echo "$merged_changes"; echo "add a new file in src/db/changes instead"; exit 1; }
echo "== linux, windows and (when its inputs changed) desktop cargo groups in parallel"
logs=$(mktemp -d)
trap 'rm -rf "$logs"' EXIT
(linux) >"$logs/linux" 2>&1 &
linux_pid=$!
(windows) >"$logs/windows" 2>&1 &
windows_pid=$!
groups="linux windows"
if [ -n "$(git diff --name-only origin/main...HEAD -- desktop api fixtures web/src web/package-lock.json rust-toolchain.toml)" ]; then
  (desktop) >"$logs/desktop" 2>&1 &
  desktop_pid=$!
  groups="$groups desktop"
fi
failed=
wait "$linux_pid" || failed="$failed linux"
wait "$windows_pid" || failed="$failed windows"
[ -z "${desktop_pid:-}" ] || wait "$desktop_pid" || failed="$failed desktop"
for group in $groups; do
  echo "== $group group output"
  cat "$logs/$group"
done
[ -z "$failed" ] || { echo "== cargo group failed:$failed"; exit 1; }
step npm --prefix web run check
step npm --prefix web run build
echo "== all checks passed"
