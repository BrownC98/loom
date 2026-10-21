#!/usr/bin/env bash
# 이 체크아웃으로 호스트(Linux)의 loom을 바꾸고 데몬을 다시 띄운다. 사람이 쓰는 loom을 올릴 때 원래 체크아웃(main)에서 부른다.
# 노드는 데몬과 따로 살아 새 데몬이 다시 붙는다. 데몬은 이 폴더를 시작 폴더로, LOOM_PORT(기본 7420)에서 듣는다.
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

pidfile=$HOME/.loom/serve.pid
# 옛 데몬만 끝낸다. 파일의 PID가 다른 프로세스로 다시 쓰였으면 건드리지 않는다.
if pid=$(cat "$pidfile" 2>/dev/null) && tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null | grep -q "^$bin serve "; then
  kill "$pid"
  while kill -0 "$pid" 2>/dev/null; do sleep 0.2; done
fi
# 새 세션이라 이 터미널을 닫아도 데몬이 산다.
setsid nohup "$bin" serve --listen "127.0.0.1:$port" >>"$HOME/.loom/serve.log" 2>&1 </dev/null 9>&- &
echo $! >"$pidfile"
for _ in $(seq 50); do
  curl -sf "http://127.0.0.1:$port/" >/dev/null && echo "$("$bin" --version): http://127.0.0.1:$port" && exit 0
  sleep 0.2
done
echo "데몬이 응답하지 않는다. $HOME/.loom/serve.log를 본다." >&2
exit 1
