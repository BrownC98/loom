#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
bin=$HOME/loom/bin/loom
port=${LOOM_PORT:-7420}
build=$HOME/.loom/host-build
target=
[ -n "${WSL_DISTRO_NAME:-}" ] && target=x86_64-pc-windows-gnu
carry=

mkdir -p "$HOME/.loom" "$(dirname "$bin")"
exec 9>"$HOME/.loom/host.lock"
flock 9

[ -e "$build/.git" ] || git clone --quiet --no-checkout . "$build"
git -C "$build" fetch --quiet "$PWD" HEAD
git -C "$build" checkout --detach --quiet --force FETCH_HEAD
(cd "$build" && bash scripts/worktree/setup.sh && export LOOM_VERSION=$(sh scripts/version.sh) LOOM_COMMIT=$(git rev-parse --short HEAD) &&
  memcap -m 8G cargo build --release ${target:+--target "$target"} &&
  if [ -n "$target" ]; then memcap -m 8G cargo build --release; fi)

if [ -z "$target" ]; then
  cp "$build/target/release/loom" "$bin.new"
  mv "$bin.new" "$bin"
  exe=$bin
else
  if [ -x "$bin" ] && [ "$(head -c 2 "$bin")" != '#!' ]; then
    "$bin" stop --listen "127.0.0.1:$port"
    carry=$HOME/.loom
  fi
  dir=$(wslpath "$(cd /mnt/c && cmd.exe /c 'echo %LOCALAPPDATA%' | tr -d '\r')")/loom
  exe=$dir/loom.exe
  mkdir -p "$dir"
  cp "$build/target/release/loom" "$dir/loom-linux.new"
  mv "$dir/loom-linux.new" "$dir/loom-linux"
  cp "$build/target/$target/release/loom.exe" "$exe.new"
  [ -e "$exe" ] && mv "$exe" "$exe.$(date +%s).old"
  mv "$exe.new" "$exe"
  rm -f "$dir"/loom.exe.*.old 2>/dev/null || true
  printf '#!/bin/sh\nexec "%s" "$@"\n' "$exe" >"$bin.new"
  chmod +x "$bin.new"
  mv "$bin.new" "$bin"
fi

"$exe" stop --listen "127.0.0.1:$port"
rm -f "$HOME/.loom/serve.pid"
if [ -n "$carry" ]; then
  data=$(wslpath "$(cd /mnt/c && cmd.exe /c 'echo %APPDATA%' | tr -d '\r')")/loom
  for f in "$data"/loom.db "$data"/loom.db-wal "$data"/loom.db-shm; do
    if [ -e "$f" ]; then mv "$f" "$f.$(date +%s).bak"; fi
  done
fi
"$exe" start --listen "127.0.0.1:$port" 9>&- && "$exe" --version
if [ -n "$carry" ]; then "$exe" carry "$carry" 9>&-; fi
