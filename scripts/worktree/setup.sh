#!/usr/bin/env bash
# 새 폴더(main 또는 Orca·loom 워크트리)에 화면 패키지를 설치하고 화면을 빌드한다. orca.yaml이 부른다.
# Rust 빌드는 빌드한 화면(web/dist)을 실행 파일에 넣으므로 cargo보다 먼저 돈다.
set -euo pipefail
web=$(dirname "${BASH_SOURCE[0]}")/../../web
npm --prefix "$web" ci
npm --prefix "$web" run build
