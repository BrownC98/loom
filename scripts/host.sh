#!/usr/bin/env bash
# 이 체크아웃으로 호스트(Linux)의 loom을 바꾸고 데몬을 다시 띄운다. 사람이 쓰는 loom을 올릴 때 원래 체크아웃(main)에서 부른다.
# 노드는 데몬과 따로 살아 새 데몬이 다시 붙는다. 데몬은 LOOM_PORT(기본 7420)에서 듣는다.
# 출력은 ~/.loom/serve.log다. 근거: docs/design.md 「배포」.
# 빌드는 이 체크아웃의 커밋(HEAD)만 쓴다. 작업 폴더의 미커밋 변경은 올라가지 않는다. 그래서 다른 작업이 커밋하지 않은 변경이 있어도 바로 올릴 수 있다.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
bin=$HOME/loom/bin/loom
port=${LOOM_PORT:-7420}
build=$HOME/.loom/host-build

# 한 번에 하나만 빌드한다. git 훅과 손으로 부른 실행이 같은 빌드 폴더를 함께 쓰면 npm 설치가 깨졌다(2026-10-21 「tsc: not found」).
# 잠금은 fd 9가 쥔다. 데몬에는 넘기지 않는다(아래 `9>&-`). 넘기면 데몬과 PTY 호스트가 잠금을 영영 쥔다.
mkdir -p "$HOME/.loom"
exec 9>"$HOME/.loom/host.lock"
flock 9

# 빌드 전용 복제를 HEAD로 옮긴다. 워크트리로 두지 않는다: 화면이 `git worktree list`를 묶음으로 보이기 때문이다.
# target 폴더를 다시 써서 빌드가 빠르다.
[ -e "$build/.git" ] || git clone --quiet --no-checkout . "$build"
git -C "$build" fetch --quiet "$PWD" HEAD
git -C "$build" checkout --detach --quiet --force FETCH_HEAD
(cd "$build" && bash scripts/worktree/setup.sh && LOOM_VERSION=$(sh scripts/version.sh) memcap -m 8G cargo build --release)
mkdir -p "$(dirname "$bin")" "$HOME/.loom"
# 새 파일로 바꿔 넣는다(rename). 도는 PTY 호스트는 옛 파일을 계속 쓴다.
cp "$build/target/release/loom" "$bin.new"
mv "$bin.new" "$bin"

# 같은 판이라도 늘 바꾼다: main 빌드는 커밋마다 판이 같을 수 있어 `start`만으로는 옛 데몬에 붙는다.
# `stop`은 데몬이 다 끝난 뒤에 돌아온다. `start`는 새 세션에 데몬을 띄워 이 터미널을 닫아도 산다.
# `daemon.json` 전의 데몬도 `stop`이 이 포트로 찾는다.
"$bin" stop --listen "127.0.0.1:$port"
rm -f "$HOME/.loom/serve.pid"
"$bin" start --listen "127.0.0.1:$port" 9>&- && "$bin" --version
