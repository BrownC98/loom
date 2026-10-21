#!/usr/bin/env bash
# 사람에게 보일 HTML 문서를 `loom show`로 점검해 열고, 렌더 그림을 남기고, 실제로 떴는지 창 제목으로 확인한다.
# 사용: show.sh <문서.html>   끝 코드 0 = 떴음, 1 = 점검 실패, 2 = 열었지만 창을 찾지 못함
set -u
doc=$(realpath "$1") || exit 1

# 1. 점검·문서 ID·열기: `loom show`가 한다(Windows 노드도 같은 명령). 형식이 틀리면 열지 않는다.
loom show "$doc" || exit 1
title=$(grep -o '<title>[^<]*' "$doc" | head -1 | sed 's/<title>//')

# 2. 렌더 점검: 헤드리스 Chromium으로 그려 그림을 남긴다. 여는 에이전트가 이 그림을 본다.
chrome=$(ls -d ~/.cache/ms-playwright/chromium-*/chrome-linux64/chrome 2>/dev/null | tail -1)
shot="${TMPDIR:-/tmp}/show-$(basename "$doc" .html).png"
if [ -n "$chrome" ]; then
  timeout 60 "$chrome" --headless --no-sandbox --disable-gpu --window-size=1440,2400 --screenshot="$shot" "file://$doc" >/dev/null 2>&1 \
    && echo "show: 렌더 그림 $shot" || echo "show: 렌더 실패(계속한다)" >&2
fi

# 3. 확인(WSL만): 새 탭이 앞에 오면 브라우저 창 제목이 문서 제목을 담는다.
if command -v powershell.exe >/dev/null; then
  for _ in $(seq 1 10); do
    sleep 1
    if (cd /mnt/c && powershell.exe -NoProfile -Command "[Console]::OutputEncoding=[Text.Encoding]::UTF8; (Get-Process | Where-Object { \$_.MainWindowTitle }).MainWindowTitle") 2>/dev/null | tr -d '\r' | grep -qF "$title"; then
      echo "show: 브라우저에 떴다: $title"; exit 0
    fi
  done
  echo "show: 열었지만 「$title」 창을 찾지 못했다. 사람에게 파일 링크를 주고 열렸는지 묻는다." >&2; exit 2
fi
echo "show: 열었다(창 확인 없음)"
