#!/usr/bin/env bash
# cargo가 Windows(x86_64-pc-windows-gnu) 실행 파일을 이 스크립트로 돌린다(`.cargo/config.toml` runner).
# WSL에서 실행 파일을 Windows 임시 폴더로 복사해 interop으로 돌린다. Wine이 아니라 실제 Windows에서 돈다.
# 근거: docs/design.md 「배포」의 Windows 개발 줄.
set -euo pipefail
exe=$1
shift
temp=$(cd /mnt/c && cmd.exe /c 'echo %TEMP%' | tr -d '\r')
dir=$(wslpath "$temp")/loom-cargo-run/$$
mkdir -p "$dir"
trap 'rm -rf "$dir"' EXIT
cp "$exe" "$dir/"
# `cargo test`가 TS 타입을 쓰는 폴더(`.cargo/config.toml` [env])를 Windows 경로로 넘긴다. 없으면 시험이 `bindings/`에 쓴다.
export WSLENV="${WSLENV:+$WSLENV:}TS_RS_EXPORT_DIR/p"
"$dir/$(basename "$exe")" "$@"
