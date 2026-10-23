#!/bin/sh
set -eu

bin=$HOME/loom/bin/loom
port=${LOOM_PORT:-7420}
raw=https://raw.githubusercontent.com/BrownC98/loom/main

if [ -n "${WSL_DISTRO_NAME:-}" ]; then
  carry=
  if [ -x "$bin" ] && [ "$(head -c 2 "$bin")" != '#!' ]; then
    "$bin" stop --listen "127.0.0.1:$port"
    carry=$HOME/.loom
  fi
  rm -f "$HOME/.loom/serve.pid"
  export WSLENV="${WSLENV:+$WSLENV:}LOOM_DOWNLOAD_URL:LOOM_DESKTOP/p"
  (cd /mnt/c && powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "[Console]::OutputEncoding = [Text.Encoding]::UTF8; irm $raw/scripts/install.ps1 | iex")
  exe=$(wslpath "$(cd /mnt/c && cmd.exe /c 'echo %LOCALAPPDATA%' | tr -d '\r')")/loom/bin/loom.exe
  mkdir -p "$(dirname "$bin")"
  printf '#!/bin/sh\nexec "%s" "$@"\n' "$exe" >"$bin.new"
  chmod +x "$bin.new"
  mv "$bin.new" "$bin"
  echo "WSL의 loom 명령이 Windows loom을 부릅니다: $bin"
  if [ -n "$carry" ]; then "$bin" carry "$carry"; fi
  exit 0
fi

case "$(uname -s)/$(uname -m)" in
Linux/x86_64) asset=loom-web-linux-x64 ;;
Darwin/*)
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
data=$HOME/.loom
mkdir -p "$(dirname "$bin")" "$data"

curl -fL --progress-bar -o "$bin.new" "$url"
chmod +x "$bin.new"
if [ "$asset" = loom-web-macos-arm64 ]; then xattr -c "$bin.new" 2>/dev/null || true; fi
mv "$bin.new" "$bin"

icon() { curl -fsL -o "$data/$1" "$raw/app/icons/$1" || rm -f "$data/$1"; }

if [ "$asset" = loom-web-macos-arm64 ]; then
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
    gio set "$desktop/loom.desktop" metadata::trusted true 2>/dev/null || true
  fi
  echo "앱 메뉴에 loom을 더했습니다: $apps/loom.desktop"
fi

pidfile=$data/serve.pid
if pid=$(cat "$pidfile" 2>/dev/null) && tr '\0' ' ' 2>/dev/null <"/proc/$pid/cmdline" | grep -q "^$bin serve "; then
  kill "$pid"
  while kill -0 "$pid" 2>/dev/null; do sleep 0.2; done
fi
rm -f "$pidfile"
"$bin" open --listen "127.0.0.1:$port"
