#!/usr/bin/env bash
# 이 폴더(main 또는 Orca 워크트리) 전용 개발 컨테이너를 띄우고 화면 패키지를 설치한다. orca.yaml이 부른다.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
# docker 그룹에 들기 전에 연 셸은 그룹 권한이 없다. 컨테이너에는 docker 그룹이 없다. 소켓이 없으면 docker compose가 그 오류를 낸다.
docker info >/dev/null 2>&1 || ! getent group docker >/dev/null || exec sg docker -c "bash $(printf %q "$PWD/scripts/worktree/setup.sh")"
# 워크트리마다 compose 프로젝트와 호스트 포트를 따로 쓴다. 폴더 이름(한국어)은 프로젝트 이름에 쓸 수 없다.
if [ ! -e .env ]; then
  port=7420
  [ -d .git ] || port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
  printf 'COMPOSE_PROJECT_NAME=loom-%s\nLOOM_PORT=%s\n' "$(pwd -P | sha1sum | cut -c1-8)" "$port" >.env
fi
# 사람마다 다른 마운트. 처음에는 호스트 docker 소켓과 loom의 기본 워크스페이스 폴더(src/worktree.rs default_workspace)를 넣는다.
# 근거: docs/design.md 「저장소와 워크트리」 「경로와 컨테이너」.
mkdir -p "$HOME/loom/workspaces"
if [ ! -e compose.override.yaml ]; then
  cat >compose.override.yaml <<YAML
services:
  dev:
    # 호스트 docker 소켓: 설정 스크립트의 docker compose가 쓴다. 호스트 root와 같은 권한이다. 지우면 끈다.
    group_add: ["$(stat -c %g /var/run/docker.sock)"]
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock
      - $HOME/loom/workspaces:$HOME/loom/workspaces
      # 다른 곳의 저장소 폴더는 호스트와 같은 경로로 한 줄씩 더한다. 예: - /srv:/srv
YAML
fi
docker compose up -d --build
docker compose exec -T -w /work/web dev npm ci
