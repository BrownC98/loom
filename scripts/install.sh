#!/bin/sh
# Linux·WSL에 loom 웹 버전을 설치하고 바탕 화면에 loom 아이콘을 만든다. 아이콘을 누르면 `loom open`이 데몬을 띄우고 브라우저를 연다.
# 사람은 한 줄로 부른다: curl -fsSL https://raw.githubusercontent.com/BrownC98/loom/main/scripts/install.sh | sh
# 다시 부르면 업데이트다: 파일을 바꿔 넣고 데몬만 다시 띄운다. 노드는 그대로 산다. 근거: docs/design.md 「배포」.
# 시험용 변수: LOOM_DOWNLOAD_URL(실행 파일 주소, file:// 가능), LOOM_DESKTOP(바탕 화면 폴더, Linux 경로), LOOM_PORT(기본 7420).
set -eu

case "$(uname -s)/$(uname -m)" in
Linux/x86_64) ;;
*)
  echo "loom 웹 버전은 x86-64 Linux·WSL만 있습니다. macOS·Windows는 데스크톱 앱을 씁니다: https://github.com/BrownC98/loom/releases/latest" >&2
  exit 1
  ;;
esac

url=${LOOM_DOWNLOAD_URL:-https://github.com/BrownC98/loom/releases/latest/download/loom-x86_64-linux}
raw=https://raw.githubusercontent.com/BrownC98/loom/main
# 노드 안의 에이전트가 `loom` 명령을 부른다. 그래서 이름은 `loom`이다.
bin=$HOME/loom/bin/loom
port=${LOOM_PORT:-7420}
data=$HOME/.loom
mkdir -p "$(dirname "$bin")" "$data"

# 새 파일로 바꿔 넣는다(rename). 도는 데몬과 노드의 PTY 호스트는 옛 파일을 계속 쓴다.
curl -fL --progress-bar -o "$bin.new" "$url"
chmod +x "$bin.new"
mv "$bin.new" "$bin"

# 아이콘을 받지 못하면 기본 아이콘으로 둔다.
icon() { curl -fsL -o "$data/$1" "$raw/app/icons/$1" || rm -f "$data/$1"; }

if [ -n "${WSL_DISTRO_NAME:-}" ]; then
  # Windows 바탕 화면에 바로 가기를 만든다. OneDrive를 쓰면 바탕 화면이 OneDrive 안에 있으므로 Windows에 묻는다.
  # 바로 가기는 최소화한 창(WindowStyle 7)으로 연다. `loom open`은 브라우저를 열고 곧 끝나므로 창이 남지 않는다.
  icon icon.ico
  if [ -n "${LOOM_DESKTOP:-}" ]; then
    desktop=$(wslpath -w "$LOOM_DESKTOP")
  else
    desktop=$(powershell.exe -NoProfile -Command "[Environment]::GetFolderPath('Desktop')" | tr -d '\r')
  fi
  icon_win=
  [ -f "$data/icon.ico" ] && icon_win=$(wslpath -w "$data/icon.ico")
  powershell.exe -NoProfile -Command "
    \$s = (New-Object -ComObject WScript.Shell).CreateShortcut('$desktop\\loom.lnk')
    \$s.TargetPath = \"\$env:SystemRoot\\System32\\wsl.exe\"
    \$s.Arguments = '-d $WSL_DISTRO_NAME -- $bin open --listen 127.0.0.1:$port'
    \$s.WindowStyle = 7
    \$s.Description = 'loom'
    if ('$icon_win') { \$s.IconLocation = '$icon_win' }
    \$s.Save()"
  printf "%s\n" "바탕 화면에 loom 아이콘을 만들었습니다: $desktop\\loom.lnk"
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

# 옛 데몬만 끝낸다. 파일의 PID가 다른 프로세스로 다시 쓰였으면 건드리지 않는다. scripts/host.sh와 같은 방법이다.
pidfile=$data/serve.pid
if pid=$(cat "$pidfile" 2>/dev/null) && tr '\0' ' ' 2>/dev/null <"/proc/$pid/cmdline" | grep -q "^$bin serve "; then
  kill "$pid"
  while kill -0 "$pid" 2>/dev/null; do sleep 0.2; done
fi
"$bin" open --listen "127.0.0.1:$port"
