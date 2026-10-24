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

if [ "$(uname -s)" = Darwin ]; then
  if [ "$(uname -m)" != arm64 ]; then
    echo "macOS 앱은 Apple Silicon만 있습니다." >&2
    exit 1
  fi
  releases=https://github.com/BrownC98/loom/releases
  url=${LOOM_DOWNLOAD_URL:-}
  if [ -z "$url" ]; then
    version=$(curl -fsSL "$releases/latest/download/latest.json" | sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p')
    url=$releases/download/v$version/loom_${version}_aarch64.app.tar.gz
  fi
  dir=/Applications
  [ -w "$dir" ] || dir=$HOME/Applications
  mkdir -p "$dir"
  tmp=$(mktemp -d "$dir/.loom.XXXXXX")
  trap 'rm -rf "$tmp"' EXIT
  curl -fL --progress-bar -o "$tmp/loom.app.tar.gz" "$url"
  tar -xzf "$tmp/loom.app.tar.gz" -C "$tmp"
  xattr -cr "$tmp/loom.app"
  quit_for=10
  if pgrep -qx loom-app; then
    osascript -e 'tell application id "io.github.brownc98.loom" to quit'
    deadline=$(($(date +%s) + quit_for))
    while pgrep -qx loom-app; do
      if [ "$(date +%s)" -ge "$deadline" ]; then
        echo "loom 앱이 끝나지 않았습니다. ⌘Q로 앱을 끝내고 다시 실행합니다." >&2
        exit 1
      fi
      sleep 0.2
    done
  fi
  if [ -e "$dir/loom.app" ]; then mv "$dir/loom.app" "$tmp/old.app"; fi
  mv "$tmp/loom.app" "$dir/loom.app"
  echo "loom 앱을 설치했습니다: $dir/loom.app"
  open "$dir/loom.app"
  exit 0
fi

if [ "$(uname -s)/$(uname -m)" != Linux/x86_64 ]; then
  echo "이 스크립트는 x86-64 Linux·WSL과 Apple Silicon macOS용입니다. Windows는 PowerShell에서 'irm https://raw.githubusercontent.com/BrownC98/loom/main/scripts/install.ps1 | iex'를 실행하거나 데스크톱 앱을 씁니다: https://github.com/BrownC98/loom/releases/latest" >&2
  exit 1
fi

url=${LOOM_DOWNLOAD_URL:-https://github.com/BrownC98/loom/releases/latest/download/loom-web-linux-x64}
data=$HOME/.loom
mkdir -p "$(dirname "$bin")" "$data"

curl -fL --progress-bar -o "$bin.new" "$url"
chmod +x "$bin.new"
mv "$bin.new" "$bin"

curl -fsL -o "$data/icon.png" "$raw/app/icons/icon.png" || rm -f "$data/icon.png"
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

pidfile=$data/serve.pid
if pid=$(cat "$pidfile" 2>/dev/null) && tr '\0' ' ' 2>/dev/null <"/proc/$pid/cmdline" | grep -q "^$bin serve "; then
  kill "$pid"
  while kill -0 "$pid" 2>/dev/null; do sleep 0.2; done
fi
rm -f "$pidfile"
"$bin" open --listen "127.0.0.1:$port"
