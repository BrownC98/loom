#!/usr/bin/env bash
set -euo pipefail
web=$(dirname "${BASH_SOURCE[0]}")/../../web
installed=$web/node_modules/.loom-package-lock.json
if ! cmp -s "$web/package-lock.json" "$installed"; then
  npm --prefix "$web" ci
  cp "$web/package-lock.json" "$installed"
fi
npm --prefix "$web" run build
