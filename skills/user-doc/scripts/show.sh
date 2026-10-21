#!/usr/bin/env bash
# 사람에게 보일 HTML 문서를 점검하고, 사람의 브라우저에 열고, 실제로 떴는지 창 제목으로 확인한다.
# 사용: show.sh <문서.html>   끝 코드 0 = 떴음, 1 = 점검 실패, 2 = 열었지만 창을 찾지 못함
set -u
doc=$(realpath "$1") || exit 1
fail() { echo "show: $*" >&2; exit 1; }

# 1. 형식 점검: style.html에서 시작한 문서인지 본다.
title=$(grep -o '<title>[^<]*' "$doc" | head -1 | sed 's/<title>//')
[ -n "$title" ] || fail "<title>이 없다"
grep -q 'style="color-scheme: light"' "$doc" || fail '<html style="color-scheme: light">가 없다(기본은 라이트)'
grep -q 'class="theme-toggle"' "$doc" || fail "라이트/다크 전환 버튼(.theme-toggle)이 없다"
grep -q '응답할 곳' "$doc" || fail "머리 줄 「응답할 곳」이 없다"
grep -q '<div class="who">' "$doc" || fail '머리 줄(<div class="who">)이 없다'

# 문서 ID: 사람이 어느 문서에 답하는지 짧게 말하게 한다. 같은 경로는 판이 바뀌어도 같은 ID다.
# 목업(docs/mockups/)은 M, 보고서는 R에 번호를 붙인다. 표는 경로와 ID 두 열이다.
ids=~/.cache/user-doc/ids.tsv
mkdir -p "$(dirname "$ids")"; touch "$ids"
id=$(awk -F'\t' -v p="$doc" '$1==p {print $2}' "$ids")
if [ -z "$id" ]; then
  case "$doc" in */docs/mockups/*) kind=M ;; *) kind=R ;; esac
  id="$kind$(( $(awk -F'\t' -v k="$kind" 'index($2,k)==1' "$ids" | wc -l) + 1 ))"
  printf '%s\t%s\n' "$doc" "$id" >> "$ids"
fi
tag="<span class=\"doc-id\">문서 ID: <b>$id</b></span>"
if grep -q 'class="doc-id"' "$doc"; then
  sed -i "s#<span class=\"doc-id\">[^<]*<b>[^<]*</b></span>#$tag#" "$doc"
else
  sed -i "s#<div class=\"who\">#&$tag#" "$doc"
fi
# 창 제목에도 ID를 붙인다. 판을 다시 열어도 한 번만 붙는다.
case "$title" in "$id "*) ;; *) title="$id $title"; sed -i "s#<title>[^<]*#<title>$title#" "$doc" ;; esac
echo "show: 문서 ID $id"

# 2. 렌더 점검: 헤드리스 Chromium으로 그려 그림을 남긴다. 사람에게 열기 전에 에이전트가 이 그림을 본다.
chrome=$(ls -d ~/.cache/ms-playwright/chromium-*/chrome-linux64/chrome 2>/dev/null | tail -1)
shot="${TMPDIR:-/tmp}/show-$(basename "$doc" .html).png"
if [ -n "$chrome" ]; then
  timeout 60 "$chrome" --headless --no-sandbox --disable-gpu --window-size=1440,2400 --screenshot="$shot" "file://$doc" >/dev/null 2>&1 \
    && echo "show: 렌더 그림 $shot" || echo "show: 렌더 실패(계속한다)" >&2
fi

# 3. 열기: WSL이면 Windows 기본 브라우저, 아니면 xdg-open.
if command -v cmd.exe >/dev/null && command -v wslpath >/dev/null; then
  win=$(wslpath -w "$doc")
  (cd /mnt/c && cmd.exe /c start "" "$win") >/dev/null 2>&1
  # 4. 확인: 새 탭이 앞에 오면 브라우저 창 제목이 문서 제목을 담는다.
  for _ in $(seq 1 10); do
    sleep 1
    if (cd /mnt/c && powershell.exe -NoProfile -Command "[Console]::OutputEncoding=[Text.Encoding]::UTF8; (Get-Process | Where-Object { \$_.MainWindowTitle }).MainWindowTitle") 2>/dev/null | tr -d '\r' | grep -qF "$title"; then
      echo "show: 브라우저에 떴다: $title"; exit 0
    fi
  done
  echo "show: 열었지만 「$title」 창을 찾지 못했다. 사람에게 파일 링크를 주고 열렸는지 묻는다." >&2; exit 2
else
  xdg-open "$doc" >/dev/null 2>&1 && echo "show: xdg-open으로 열었다(창 확인 없음)" && exit 0
  exit 2
fi
