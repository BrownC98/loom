#!/usr/bin/env bash
# 사람에게 보일 HTML 문서를 `loom show`로 점검하고 문서 ID를 붙이고, 렌더 그림을 남긴다. 사람의 브라우저는 열지 않는다.
# 사람에게는 `loom ask "<질문>" --doc <문서>`로 질문에 붙여 보인다.
# 사용: show.sh <문서.html>   끝 코드 0 = 점검 통과, 1 = 점검 실패
set -u
doc=$(realpath "$1") || exit 1

# 1. 점검·문서 ID: `loom show`가 한다(Windows 노드도 같은 명령).
loom show "$doc" || exit 1

# 2. 렌더 점검: 헤드리스 Chromium으로 그려 그림을 남긴다. 묻는 에이전트가 이 그림을 본다.
chrome=$(ls -d ~/.cache/ms-playwright/chromium-*/chrome-linux64/chrome 2>/dev/null | tail -1)
shot="${TMPDIR:-/tmp}/show-$(basename "$doc" .html).png"
if [ -n "$chrome" ]; then
  timeout 60 "$chrome" --headless --no-sandbox --disable-gpu --window-size=1440,2400 --screenshot="$shot" "file://$doc" >/dev/null 2>&1 \
    && echo "show: 렌더 그림 $shot" || echo "show: 렌더 실패(계속한다)" >&2
fi

# 3. 목업 말풍선 점검: 말풍선이 있는 목업이면 겹침과 잘리거나 가려진 번호 배지를 센다(SKILL.md 「목업 말풍선」).
if grep -q 'class="calls"' "$doc"; then
  out=$(timeout 120 node "$(dirname "$0")/check-callouts.cjs" "$doc") || { echo "show: 말풍선 점검을 돌리지 못했다: $out" >&2; exit 1; }
  echo "show: 말풍선 점검 $out"
  case $out in *'"overlaps":[]'*) ;; *) exit 1 ;; esac
fi
