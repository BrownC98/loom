#!/bin/sh
# Linux·WSL·macOS에 loom 웹 버전을 설치하고 loom 아이콘을 만든다. 아이콘을 누르면 `loom open`이 데몬을 띄우고 브라우저를 연다.
# 사람은 한 줄로 부른다: curl -fsSL https://raw.githubusercontent.com/BrownC98/loom/main/scripts/install.sh | sh
# 다시 부르면 업데이트다: 파일을 바꿔 넣고 옛 판 데몬만 새 판으로 바꾼다. 노드는 그대로 산다. 그 뒤 업데이트는 화면의 [업데이트]로도 한다.
# macOS: 데스크톱 앱이 있으면 앱 안의 loom을 여는 바로 가기만 만든다. 없으면 웹 버전 실행 파일을 받는다. 근거: docs/design.md 「배포」.
# WSL: Windows loom을 설치한다(install.ps1). Windows 데몬 하나가 WSL 노드도 돌린다(docs/design.md 「데몬 하나」).
# 시험용 변수: LOOM_DOWNLOAD_URL(실행 파일 주소, file:// 가능. WSL은 Windows가 읽는 주소), LOOM_DESKTOP(바탕 화면 폴더, Linux 경로), LOOM_PORT(기본 7420).
set -eu

bin=$HOME/loom/bin/loom
port=${LOOM_PORT:-7420}
raw=https://raw.githubusercontent.com/BrownC98/loom/main

if [ -n "${WSL_DISTRO_NAME:-}" ]; then
  # 옛 WSL 데몬을 끝낸다. 그래야 Windows 데몬이 같은 포트를 쓴다. 껍데기(`#!`)면 이미 바꿨다.
  if [ -x "$bin" ] && [ "$(head -c 2 "$bin")" != '#!' ]; then "$bin" stop --listen "127.0.0.1:$port"; fi
  rm -f "$HOME/.loom/serve.pid"
  # 시험용 변수는 WSLENV로 넘긴다. LOOM_DESKTOP은 Windows 경로로 바꾼다(/p).
  export WSLENV="${WSLENV:+$WSLENV:}LOOM_DOWNLOAD_URL:LOOM_DESKTOP/p"
  # 시작 폴더가 WSL 폴더(UNC)면 Windows 명령이 경고를 낸다.
  (cd /mnt/c && powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "[Console]::OutputEncoding = [Text.Encoding]::UTF8; irm $raw/scripts/install.ps1 | iex")
  # WSL 터미널의 `loom`도 Windows loom을 부른다. Linux loom을 따로 띄우면 데몬이 둘이 된다.
  exe=$(wslpath "$(cd /mnt/c && cmd.exe /c 'echo %LOCALAPPDATA%' | tr -d '\r')")/loom/bin/loom.exe
  mkdir -p "$(dirname "$bin")"
  printf '#!/bin/sh\nexec "%s" "$@"\n' "$exe" >"$bin.new"
  chmod +x "$bin.new"
  mv "$bin.new" "$bin"
  echo "WSL의 loom 명령이 Windows loom을 부릅니다: $bin"
  exit 0
fi

case "$(uname -s)/$(uname -m)" in
Linux/x86_64) asset=loom-web-linux-x64 ;;
Darwin/*)
  # 데스크톱 앱이 있으면 앱 안의 loom을 쓴다. 두 번째 실행 파일을 받지 않는다: 같은 데몬을 앱과 브라우저가 함께 본다.
  # 앱이 띄운 데몬이 적은 실행 파일을 먼저 쓰고, 없으면 기본 설치 자리를 본다. 웹 버전 데몬이 적은 `~/loom/bin/loom`은 앱이 아니다.
  exe=$(sed -n 's/.*"exe":"\([^"]*\)".*/\1/p' "$HOME/.loom/daemon.json" 2>/dev/null || true)
  case "$exe" in
  */Contents/MacOS/loom) ;;
  *) exe=/Applications/loom.app/Contents/MacOS/loom ;;
  esac
  if [ -x "$exe" ]; then
    launcher="$HOME/Applications/loom (browser).command"
    mkdir -p "$HOME/Applications"
    printf '#!/bin/sh\nexec "%s" open\n' "$exe" >"$launcher"
    chmod +x "$launcher"
    echo "브라우저로 여는 loom을 만들었습니다: $launcher"
    exec "$exe" open
  fi
  # 앱이 없으면 웹 버전 실행 파일을 받는다. Release에는 Apple Silicon 파일만 있다.
  if [ "$(uname -m)" != arm64 ]; then
    echo "macOS 웹 버전은 Apple Silicon만 있습니다." >&2
    exit 1
  fi
  asset=loom-web-macos-arm64
  ;;
*)
  echo "이 스크립트는 x86-64 Linux·WSL·macOS용입니다. Windows는 PowerShell에서 'irm https://raw.githubusercontent.com/BrownC98/loom/main/scripts/install.ps1 | iex'를 실행하거나 데스크톱 앱을 씁니다: https://github.com/BrownC98/loom/releases/latest" >&2
  exit 1
  ;;
esac

url=${LOOM_DOWNLOAD_URL:-https://github.com/BrownC98/loom/releases/latest/download/$asset}
# 노드 안의 에이전트가 `loom` 명령을 부른다. 그래서 이름은 `loom`이다.
data=$HOME/.loom
mkdir -p "$(dirname "$bin")" "$data"

# 새 파일로 바꿔 넣는다(rename). 도는 데몬과 노드의 PTY 호스트는 옛 파일을 계속 쓴다.
curl -fL --progress-bar -o "$bin.new" "$url"
chmod +x "$bin.new"
# macOS는 받은 파일 표시가 있으면 실행을 막는다. curl은 표시를 붙이지 않지만 다른 길로 받은 파일도 같게 둔다.
if [ "$asset" = loom-web-macos-arm64 ]; then xattr -c "$bin.new" 2>/dev/null || true; fi
mv "$bin.new" "$bin"

# 아이콘을 받지 못하면 기본 아이콘으로 둔다.
icon() { curl -fsL -o "$data/$1" "$raw/app/icons/$1" || rm -f "$data/$1"; }

if [ "$asset" = loom-web-macos-arm64 ]; then
  # 응용 프로그램 폴더의 `.command`를 누르면 터미널이 `loom open`을 부른다. 브라우저를 연 뒤 끝난다.
  launcher="$HOME/Applications/loom.command"
  mkdir -p "$HOME/Applications"
  printf '#!/bin/sh\nexec "%s" open --listen 127.0.0.1:%s\n' "$bin" "$port" >"$launcher"
  chmod +x "$launcher"
  echo "loom 아이콘을 만들었습니다: $launcher"
else
  icon icon.png
  apps=$HOME/.local/share/applications
  mkdir -p "$apps"
  cat >"$apps/loom.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=loom
Comment=Open loom in the browser
Exec=$bin open --listen 127.0.0.1:$port
Icon=$data/icon.png
Terminal=false
Categories=Development;
EOF
  desktop=${LOOM_DESKTOP:-$(xdg-user-dir DESKTOP 2>/dev/null || echo "$HOME/Desktop")}
  if [ -d "$desktop" ]; then
    cp "$apps/loom.desktop" "$desktop/loom.desktop"
    chmod +x "$desktop/loom.desktop"
    # GNOME은 신뢰 표시가 없는 바탕 화면 파일을 실행하지 않는다.
    gio set "$desktop/loom.desktop" metadata::trusted true 2>/dev/null || true
  fi
  echo "앱 메뉴에 loom을 더했습니다: $apps/loom.desktop"
fi

# `daemon.json` 전의 데몬(serve.pid)을 끝낸다. 그 데몬은 잠금이 없어 `open`이 찾지 못한다. 파일의 PID가 다른 프로세스로 다시 쓰였으면 건드리지 않는다.
pidfile=$data/serve.pid
if pid=$(cat "$pidfile" 2>/dev/null) && tr '\0' ' ' 2>/dev/null <"/proc/$pid/cmdline" | grep -q "^$bin serve "; then
  kill "$pid"
  while kill -0 "$pid" 2>/dev/null; do sleep 0.2; done
fi
rm -f "$pidfile"
# 도는 데몬이 옛 판이면 `open`이 새 판으로 바꾼다. 같은 판이나 새 판이면 붙는다.
"$bin" open --listen "127.0.0.1:$port"
