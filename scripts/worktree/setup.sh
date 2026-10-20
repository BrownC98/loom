#!/usr/bin/env bash
# 이 폴더(main 또는 Orca 워크트리) 전용 개발 컨테이너를 띄우고 화면 패키지를 설치한다. orca.yaml이 부른다.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
# docker 그룹에 들기 전에 연 셸은 그룹 권한이 없다.
docker info >/dev/null 2>&1 || exec sg docker -c "bash $(printf %q "$PWD/scripts/worktree/setup.sh")"
# 워크트리마다 compose 프로젝트와 호스트 포트를 따로 쓴다. 폴더 이름(한국어)은 프로젝트 이름에 쓸 수 없다.
if [ ! -e .env ]; then
  port=7420
  [ -d .git ] || port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
  printf 'COMPOSE_PROJECT_NAME=loom-%s\nLOOM_PORT=%s\n' "$(pwd -P | sha1sum | cut -c1-8)" "$port" >.env
fi
grep -q '^DOCKER_GID=' .env || echo "DOCKER_GID=$(stat -c %g /var/run/docker.sock)" >>.env
docker compose up -d --build
docker compose exec -T -w /work/web dev npm ci
