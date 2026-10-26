#!/usr/bin/env bash
set -euo pipefail
exe=$1
shift
temp=$(cd /mnt/c && cmd.exe /c 'echo %TEMP%' | tr -d '\r')
dir=$(wslpath "$temp")/loom-cargo-run/$$
mkdir -p "$dir"
trap 'rm -rf "$dir"' EXIT
cp "$exe" "$dir/"
export TS_RS_EXPORT_DIR="$temp\\loom-cargo-run\\$$\\gen"
export WSLENV="${WSLENV:+$WSLENV:}TS_RS_EXPORT_DIR"
"$dir/$(basename "$exe")" "$@"
