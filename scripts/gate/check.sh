#!/usr/bin/env bash
# 병합 관문 검사: `loom merge`가 담당 브랜치 워크트리에서 부른다. 하나라도 실패하면 0이 아닌 코드로 끝나 담당에게 반려가 간다.
# 목록은 AGENTS.md 「개발 환경」 표와 같다. 손으로 돌려 볼 수 있다: 워크트리 폴더에서 `bash scripts/gate/check.sh`.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

# VM 메모리(22GB)가 바닥나면 모든 세션이 꺼진다. 무거운 cargo 단계는 여유가 8GiB 넘을 때만 띄운다(AGENTS.md 「메모리 안전」).
capacity=$HOME/.agents/skills/dev-toolbox/scripts/capacity.py
room() {
  [ -f "$capacity" ] || return 0
  local waited=0
  until [ "$(python3 "$capacity" claude 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["headroomMiB"] >= 8192)' 2>/dev/null)" = True ]; do
    # 40분 기다려도 자리가 나지 않으면 환경 문제다. 검사 실패로 끝내 사람이 본다.
    [ "$waited" -ge 2400 ] && { echo "no memory headroom for 40 min"; return 1; }
    sleep 60
    waited=$((waited + 60))
  done
}
step() { echo "== $*"; "$@"; }

[ -d web/node_modules ] && [ -d web/dist ] || step timeout 600 bash scripts/worktree/setup.sh
step cargo fmt --check
room; step memcap -m 8G cargo clippy --all-targets
room; step memcap -m 8G cargo test
room; step memcap -m 8G cargo clippy --all-targets --target x86_64-pc-windows-gnu
room; step memcap -m 8G cargo test --target x86_64-pc-windows-gnu
step npm --prefix web run check
step npm --prefix web test
step npm --prefix web run build
echo "== all checks passed"
