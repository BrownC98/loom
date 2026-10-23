#!/usr/bin/env bash
set -euo pipefail
web=$(dirname "${BASH_SOURCE[0]}")/../../web
npm --prefix "$web" ci
npm --prefix "$web" run build
