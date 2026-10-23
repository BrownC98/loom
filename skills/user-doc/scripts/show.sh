#!/usr/bin/env bash
# 사람에게 보일 HTML 문서를 점검하고, 렌더 그림을 남기고, `loom show`로 문서 ID를 붙인다. 사람의 브라우저는 열지 않는다.
# 사람에게는 `loom ask "<질문>" --doc <문서>`로 질문에 붙여 보인다. `--doc`은 이 점검을 모두 지난 뒤 바뀌지 않은 파일만 받는다.
# 사용: show.sh <문서.html>   끝 코드 0 = 점검 통과, 1 = 점검 실패
set -u
doc=$(realpath "$1") || exit 1

# 1. 렌더 점검: 헤드리스 Chromium으로 그려 그림을 남긴다. 묻는 에이전트가 이 그림을 본다.
chrome=$(ls -d ~/.cache/ms-playwright/chromium-*/chrome-linux64/chrome 2>/dev/null | tail -1)
shot="${TMPDIR:-/tmp}/show-$(basename "$doc" .html).png"
if [ -n "$chrome" ]; then
  timeout 60 "$chrome" --headless --no-sandbox --disable-gpu --window-size=1440,2400 --screenshot="$shot" "file://$doc" >/dev/null 2>&1 \
    && echo "show: 렌더 그림 $shot" || echo "show: 렌더 실패(계속한다)" >&2
fi

# 2. 파일 하나와 목업 말풍선 점검(SKILL.md 「형식」·「목업 말풍선」). 창 폭 7가지로 돌아 수십 초 걸린다.
timeout 300 node "$(dirname "$0")/check-callouts.cjs" "$doc" || { echo "show: 문서 점검 실패(위 overlaps)" >&2; exit 1; }

# 3. 형식 점검·문서 ID·통과 기록: `loom show`가 한다(Windows 노드는 이 명령만 쓴다). 앞 점검이 모두 지난 뒤에만 부른다.
loom show "$doc" || exit 1
