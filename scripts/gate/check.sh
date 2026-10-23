#!/usr/bin/env bash
# 병합 관문 검사: `loom merge`가 담당 브랜치 워크트리에서 부른다. 하나라도 실패하면 0이 아닌 코드로 끝나 담당에게 반려가 간다.
# 목록은 AGENTS.md 「개발 환경」 표와 같다. 손으로 돌려 볼 수 있다: 워크트리 폴더에서 `bash scripts/gate/check.sh`.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

# VM 메모리(22GB)가 바닥나면 모든 세션이 꺼진다. 무거운 cargo 단계는 여유가 상한 넘게 있을 때만 띄운다.
# 실측(2026-10-23, 단계마다 target 비운 첫 빌드 2회, cgroup memory.peak): clippy 2159·2239, test 2576·2689,
# Windows clippy 2559·2574, Windows test 3008·3076 MiB. 이미 빌드한 target은 모두 130 MiB 이하.
# 최대 3076에 여유 약 65%를 더해 5GiB. 상한(memcap)과 기다릴 여유를 같은 값으로 둔다. 8GiB는 실측의 2.6배라 검사가 37분 넘게 굶었다.
CARGO_STEP_MIB=5120
capacity=$HOME/.agents/skills/dev-toolbox/scripts/capacity.py
room() {
  [ -f "$capacity" ] || return 0
  local waited=0
  until [ "$(python3 "$capacity" claude 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["headroomMiB"] >= '"$CARGO_STEP_MIB"')' 2>/dev/null)" = True ]; do
    # 40분 기다려도 자리가 나지 않으면 환경 문제다. 검사 실패로 끝내 사람이 본다.
    [ "$waited" -ge 2400 ] && { echo "no memory headroom for 40 min"; return 1; }
    sleep 60
    waited=$((waited + 60))
  done
}
step() { echo "== $*"; "$@"; }

[ -d web/node_modules ] && [ -d web/dist ] || step timeout 600 bash scripts/worktree/setup.sh
step cargo fmt --check
room; step memcap -m "${CARGO_STEP_MIB}M" cargo clippy --all-targets
room; step memcap -m "${CARGO_STEP_MIB}M" cargo test
room; step memcap -m "${CARGO_STEP_MIB}M" cargo clippy --all-targets --target x86_64-pc-windows-gnu
room; step memcap -m "${CARGO_STEP_MIB}M" cargo test --target x86_64-pc-windows-gnu
step npm --prefix web run check
step npm --prefix web test
step npm --prefix web run build
echo "== all checks passed"
