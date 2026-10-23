#!/usr/bin/env bash
# 이 체크아웃으로 사람이 쓰는 loom을 바꾸고 데몬을 다시 띄운다. 원래 체크아웃(main)에서 부른다.
# Linux: `~/loom/bin/loom`. WSL: 이 PC의 Windows loom(앱 옆 `%LOCALAPPDATA%\loom\loom.exe`)이다. Windows 데몬 하나가 WSL 노드도 돌린다.
# 노드는 데몬과 따로 살아 새 데몬이 다시 붙는다. 데몬은 LOOM_PORT(기본 7420)에서 듣는다. 근거: docs/design.md 「배포」.
# 빌드는 이 체크아웃의 커밋(HEAD)만 쓴다. 작업 폴더의 미커밋 변경은 올라가지 않는다. 그래서 다른 작업이 커밋하지 않은 변경이 있어도 바로 올릴 수 있다.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
bin=$HOME/loom/bin/loom
port=${LOOM_PORT:-7420}
build=$HOME/.loom/host-build
target=
[ -n "${WSL_DISTRO_NAME:-}" ] && target=x86_64-pc-windows-gnu
carry=

# 한 번에 하나만 빌드한다. git 훅과 손으로 부른 실행이 같은 빌드 폴더를 함께 쓰면 npm 설치가 깨졌다(2026-10-21 「tsc: not found」).
# 잠금은 fd 9가 쥔다. 데몬에는 넘기지 않는다(아래 `9>&-`). 넘기면 데몬과 PTY 호스트가 잠금을 영영 쥔다.
mkdir -p "$HOME/.loom" "$(dirname "$bin")"
exec 9>"$HOME/.loom/host.lock"
flock 9

# 빌드 전용 복제를 HEAD로 옮긴다. 워크트리로 두지 않는다: 화면이 `git worktree list`를 묶음으로 보이기 때문이다.
# target 폴더를 다시 써서 빌드가 빠르다.
[ -e "$build/.git" ] || git clone --quiet --no-checkout . "$build"
git -C "$build" fetch --quiet "$PWD" HEAD
git -C "$build" checkout --detach --quiet --force FETCH_HEAD
(cd "$build" && bash scripts/worktree/setup.sh && LOOM_VERSION=$(sh scripts/version.sh) LOOM_COMMIT=$(git rev-parse --short HEAD) memcap -m 8G cargo build --release ${target:+--target "$target"})

if [ -z "$target" ]; then
  # 새 파일로 바꿔 넣는다(rename). 도는 PTY 호스트는 옛 파일을 계속 쓴다.
  cp "$build/target/release/loom" "$bin.new"
  mv "$bin.new" "$bin"
  exe=$bin
else
  # 옛 WSL 데몬을 끝낸다. 그래야 Windows 데몬이 같은 포트를 쓴다. 껍데기(`#!`)면 이미 바꿨다.
  if [ -x "$bin" ] && [ "$(head -c 2 "$bin")" != '#!' ]; then
    "$bin" stop --listen "127.0.0.1:$port"
    carry=$HOME/.loom
  fi
  # Windows 환경 변수는 cmd.exe로 읽는다. 시험은 APPDATA·LOCALAPPDATA를 WSLENV로 넘겨 임시 폴더를 쓴다.
  dir=$(wslpath "$(cd /mnt/c && cmd.exe /c 'echo %LOCALAPPDATA%' | tr -d '\r')")/loom
  exe=$dir/loom.exe
  mkdir -p "$dir"
  # 도는 실행 파일은 덮어 쓰지 못하지만 이름은 바꿀 수 있다. 도는 PTY 호스트는 옛 파일을 계속 쓴다(「데스크톱 앱」 설치 줄과 같다).
  [ -e "$exe" ] && mv "$exe" "$exe.$(date +%s).old"
  cp "$build/target/$target/release/loom.exe" "$exe"
  rm -f "$dir"/loom.exe.*.old 2>/dev/null || true
  # WSL 터미널의 `loom`도 Windows loom을 부른다. Linux loom을 따로 띄우면 데몬이 둘이 된다.
  printf '#!/bin/sh\nexec "%s" "$@"\n' "$exe" >"$bin.new"
  chmod +x "$bin.new"
  mv "$bin.new" "$bin"
fi

# 같은 판이라도 늘 바꾼다: main 빌드는 커밋마다 판이 같을 수 있어 `start`만으로는 옛 데몬에 붙는다.
# `stop`은 데몬이 다 끝난 뒤에 돌아온다. `start`는 새 세션에 데몬을 띄워 이 터미널을 닫아도 산다.
# `daemon.json` 전의 데몬도 `stop`이 이 포트로 찾는다.
"$exe" stop --listen "127.0.0.1:$port"
rm -f "$HOME/.loom/serve.pid"
# 옮기기는 저장소·노드가 없는 DB에만 한다(노드·질문 번호를 그대로 둔다). 앞서 Windows 앱이 쓴 DB는 이름을 바꿔 남긴다.
# 2026-10-22 사람 결정(총괄 확인: 그 DB는 WSL에도 있는 저장소 1개, 노드 0개).
if [ -n "$carry" ]; then
  data=$(wslpath "$(cd /mnt/c && cmd.exe /c 'echo %APPDATA%' | tr -d '\r')")/loom
  for f in "$data"/loom.db "$data"/loom.db-wal "$data"/loom.db-shm; do
    if [ -e "$f" ]; then mv "$f" "$f.$(date +%s).bak"; fi
  done
fi
"$exe" start --listen "127.0.0.1:$port" 9>&- && "$exe" --version
# 옛 WSL 데몬의 노드를 Windows 데몬으로 옮겨 같은 세션으로 다시 띄운다. 옛 노드를 끝내므로 이 스크립트를 부른 노드도 여기서 끝날 수 있다.
# 옮기기는 데몬이 끝까지 한다. 결과는 데몬 로그와 다시 뜬 노드의 첫 입력에 남는다.
if [ -n "$carry" ]; then "$exe" carry "$carry" 9>&-; fi
