#!/usr/bin/env bash
# 이 체크아웃(HEAD)으로 이 PC의 Windows loom(앱 옆 `%LOCALAPPDATA%\loom\loom.exe`)을 바꾸고 Windows 데몬을 다시 띄운다.
# WSL에서 원래 체크아웃(main)이 부른다. 빌드는 x86_64-pc-windows-gnu(mingw-w64)다. 노드는 데몬과 따로 살아 새 데몬이 다시 붙는다.
# 데몬은 LOOM_WINDOWS_PORT(기본 7421)에서 듣는다. WSL 데몬이 7420을 쓰는 동안 둘이 함께 돈다.
# 근거: docs/design.md 「배포」.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
port=${LOOM_WINDOWS_PORT:-7421}
build=$HOME/.loom/host-build
target=x86_64-pc-windows-gnu

# 빌드 폴더와 잠금은 scripts/host.sh와 같다. target 폴더를 함께 써서 빌드가 빠르다.
mkdir -p "$HOME/.loom"
exec 9>"$HOME/.loom/host.lock"
flock 9
[ -e "$build/.git" ] || git clone --quiet --no-checkout . "$build"
git -C "$build" fetch --quiet "$PWD" HEAD
git -C "$build" checkout --detach --quiet --force FETCH_HEAD
(cd "$build" && bash scripts/worktree/setup.sh && LOOM_VERSION=$(sh scripts/version.sh) memcap -m 8G cargo build --release --target "$target")

# Windows 환경 변수는 cmd.exe로 읽는다. 시험은 APPDATA·LOCALAPPDATA를 WSLENV로 넘겨 임시 폴더를 쓴다.
local_app=$(cd /mnt/c && cmd.exe /c 'echo %LOCALAPPDATA%' | tr -d '\r')
dir=$(wslpath "$local_app")/loom
exe=$dir/loom.exe
mkdir -p "$dir"
# 도는 실행 파일은 덮어 쓰지 못하지만 이름은 바꿀 수 있다. 도는 PTY 호스트는 옛 파일을 계속 쓴다(「데스크톱 앱」 설치 줄과 같다).
[ -e "$exe" ] && mv "$exe" "$exe.$(date +%s).old"
cp "$build/target/$target/release/loom.exe" "$exe"
rm -f "$dir"/loom.exe.*.old 2>/dev/null || true

# 같은 판이라도 늘 바꾼다(scripts/host.sh와 같은 이유). 데몬은 9번 fd를 받지 않는다.
"$exe" stop
"$exe" start --listen "127.0.0.1:$port" 9>&- && "$exe" --version
