#!/usr/bin/env bash
set -euo pipefail
unset $(git rev-parse --local-env-vars)
cd "$(dirname "${BASH_SOURCE[0]}")/.."
bin=$HOME/loom/bin/loom
port=${LOOM_PORT:-7420}
build=$HOME/.loom/host-build
deployed=$HOME/.loom/host-deployed
target=
[ -n "${WSL_DISTRO_NAME:-}" ] && target=x86_64-pc-windows-gnu
carry=

mkdir -p "$HOME/.loom" "$(dirname "$bin")"
exec 9>"$HOME/.loom/host.lock"
flock 9

[ -e "$build/.git" ] || git clone --quiet --no-checkout . "$build"
git -C "$build" fetch --quiet "$PWD" HEAD
head=$(git -C "$build" rev-parse FETCH_HEAD)
[ "$head" != "$(cat "$deployed" 2>/dev/null)" ] || exit 0
git -C "$build" checkout --detach --quiet --force "$head"
version=$(cd "$build" && sh scripts/version.sh)
data=$HOME/.loom
exe=$bin
if [ -n "$target" ]; then
  appdata=$(cd /mnt/c && cmd.exe /c 'echo %APPDATA%' | tr -d '\r')
  [ -n "$appdata" ] || { echo "host.sh: cmd.exe did not give %APPDATA%" >&2; exit 1; }
  data=$(wslpath "$appdata")/loom
  dir=$(wslpath "$(cd /mnt/c && cmd.exe /c 'echo %LOCALAPPDATA%' | tr -d '\r')")/loom
  exe=$dir/loom.exe
  if [ -x "$bin" ] && [ "$(head -c 2 "$bin")" != '#!' ]; then carry=$HOME/.loom; fi
fi
answers() {
  local code=0
  "$1" -s --noproxy '*' --max-time 2 "$2/" >/dev/null || code=$?
  case $code in
    0) return 0 ;;
    7) return 1 ;;
  esac
  echo "host.sh: $1 $2/ ended with code $code. host.sh cannot tell if a daemon runs there, so it does not stop or replace anything." >&2
  exit 1
}
linuxpath() { readlink -m "$1"; }
windowspath() {
  local p=$1
  case $p in [A-Za-z]:*) p=$(wslpath -u "$p") ;; esac
  p=$(readlink -m "$p")
  printf '%s\n' "${p,,}"
}
check() {
  local probe=$1 data=$2 exe=$3 path=$4 live url running at
  live=$(cat "$data/daemon.json" 2>/dev/null || true)
  field() { printf '%s' "$live" | sed -n "s/.*\"$1\":\"\([^\"]*\)\".*/\1/p"; }
  url=$(field url)
  if [ "$url" != "http://127.0.0.1:$port" ] && answers "$probe" "http://127.0.0.1:$port"; then
    echo "host.sh: a daemon answers at 127.0.0.1:$port, but $data/daemon.json does not describe it. host.sh does not stop an unknown daemon. Stop it yourself, then run host.sh again." >&2
    exit 1
  fi
  if [ -z "$url" ] || ! answers "$probe" "$url"; then return 0; fi
  running=$(field version)
  at=$(field exe | sed 's/\\\\/\\/g')
  if [ "$("$path" "$at")" != "$("$path" "$exe")" ]; then
    echo "host.sh: the daemon at $url runs $at, not $exe. host.sh does not stop or replace a daemon that it did not deploy. Stop it yourself, then run host.sh again." >&2
    exit 1
  fi
  if [ "$running" != "$version" ] && [ "$(printf '%s\n' "$running" "$version" | sort -V | tail -n 1)" = "$running" ]; then
    echo "host.sh: the daemon at $url is loom $running, newer than this build ($version). host.sh does not replace it with an older loom." >&2
    exit 1
  fi
}
guard() {
  if [ -z "$target" ]; then
    check curl "$data" "$exe" linuxpath
    return
  fi
  check curl.exe "$data" "$exe" windowspath
  if [ -n "$carry" ]; then check curl "$carry" "$bin" linuxpath; fi
}
guard
linux=
if [ -n "$target" ] && command -v x86_64-linux-musl-gcc >/dev/null; then linux=x86_64-unknown-linux-musl; fi
(cd "$build" && bash scripts/worktree/setup.sh && export LOOM_VERSION=$version LOOM_COMMIT=$(git rev-parse --short HEAD) LOOM_RELEASES=https://github.com/BrownC98/loom/releases &&
  memcap -m 3584M cargo build --release ${target:+--target "$target"} &&
  if [ -n "$linux" ]; then rustup target add "$linux" && memcap -m 3584M cargo build --release --target "$linux"; fi &&
  if [ -n "$target" ] && [ -z "$linux" ]; then memcap -m 3584M cargo build --release; fi)

guard
if [ -z "$target" ]; then
  cp "$build/target/release/loom" "$bin.new"
  mv "$bin.new" "$bin"
else
  if [ -n "$carry" ]; then "$bin" stop --listen "127.0.0.1:$port"; fi
  mkdir -p "$dir"
  cp "$build/target/${linux:+$linux/}release/loom" "$dir/loom-linux.new"
  mv "$dir/loom-linux.new" "$dir/loom-linux"
  cp "$build/target/$target/release/loom.exe" "$exe.new"
  [ -e "$exe" ] && mv "$exe" "$exe.$(date +%s).old"
  mv "$exe.new" "$exe"
  rm -f "$dir"/loom.exe.*.old 2>/dev/null || true
  printf '#!/bin/sh\ncase $1 in serve|start|stop|open) [ "$HOME" = '"'%s'"' ] || { echo "loom: this command would act on the human'"'"'s Windows daemon; build a Linux test binary instead (see AGENTS.md \\"데몬 띄우기(시험)\\")" >&2; exit 2; } ;; esac\nexec "%s" "$@"\n' "$(printf %s "$HOME" | sed "s/'/'\\\\''/g")" "$exe" >"$bin.new"
  chmod +x "$bin.new"
  mv "$bin.new" "$bin"
fi

"$exe" stop --listen "127.0.0.1:$port"
rm -f "$HOME/.loom/serve.pid"
if [ -n "$carry" ]; then
  for f in "$data"/loom.db "$data"/loom.db-wal "$data"/loom.db-shm; do
    if [ -e "$f" ]; then mv "$f" "$f.$(date +%s).bak"; fi
  done
fi
owner=$HOME/.loom
if [ -n "$target" ]; then owner="$appdata\\loom"; fi
mkdir -p "$HOME/.cache/loom"
printf '%s\n' "$owner" >"$HOME/.cache/loom/.loom-owner"
"$exe" start --listen "127.0.0.1:$port" 9>&- && "$exe" --version && echo "$head" >"$deployed"
if [ -n "$carry" ]; then "$exe" carry "$carry" 9>&-; fi
