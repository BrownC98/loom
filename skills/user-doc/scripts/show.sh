#!/usr/bin/env bash
set -u
doc=$(realpath "$1") || exit 1

loom show "$doc" || exit 1

chrome=$(ls -d ~/.cache/ms-playwright/chromium-*/chrome-linux64/chrome 2>/dev/null | tail -1)
shot="${TMPDIR:-/tmp}/show-$(basename "$doc" .html).png"
if [ -n "$chrome" ]; then
  timeout 60 "$chrome" --headless --no-sandbox --disable-gpu --window-size=1440,2400 --screenshot="$shot" "file://$doc" >/dev/null 2>&1 \
    && echo "show: 렌더 그림 $shot" || echo "show: 렌더 실패(계속한다)" >&2
fi

timeout 300 node "$(dirname "$0")/check-callouts.cjs" "$doc" || { echo "show: 문서 점검 실패(위 overlaps)" >&2; exit 1; }

loom show --checked "$doc" || exit 1
