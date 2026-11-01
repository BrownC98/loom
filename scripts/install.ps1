$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$url = if ($env:LOOM_DOWNLOAD_URL) { $env:LOOM_DOWNLOAD_URL } else { 'https://github.com/BrownC98/loom/releases/latest/download/loom-web-windows-x64.exe' }
$linux_url = $url -replace 'loom-web-windows-x64\.exe$', 'loom-web-linux-x64'
$desktop_url = $url -replace 'loom-web-windows-x64\.exe$', 'loom-desktop-windows-x64.zip'
$raw = 'https://raw.githubusercontent.com/BrownC98/loom/main'
$home_dir = Join-Path $env:LOCALAPPDATA 'loom'
$dir = Join-Path $home_dir 'bin'
$exe = Join-Path $dir 'loom.exe'
$app = Join-Path $dir 'loom-desktop.exe'
$linux = Join-Path $dir 'loom-linux'
$unpack = Join-Path $home_dir 'desktop.new'
New-Item -ItemType Directory -Force -Path $dir | Out-Null

function Install-File($from, $to) {
  if (Test-Path $to) {
    try {
      Remove-Item $to -Force
    } catch {
      Rename-Item $to "$(Split-Path $to -Leaf).$([DateTimeOffset]::UtcNow.ToUnixTimeSeconds()).old"
    }
  }
  Move-Item $from $to
}

Get-ChildItem -Path $dir -Filter '*.exe.*.old' -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
Remove-Item $unpack -Recurse -Force -ErrorAction SilentlyContinue
$with_linux = $linux_url -ne $url
if ($with_linux) { Invoke-WebRequest -Uri $linux_url -OutFile "$linux.new" -UseBasicParsing }

$with_desktop = $false
if ($desktop_url -ne $url) {
  try {
    Invoke-WebRequest -Uri $desktop_url -OutFile "$unpack.zip" -UseBasicParsing
    Expand-Archive -Path "$unpack.zip" -DestinationPath $unpack -Force
    $with_desktop = (Test-Path (Join-Path $unpack '*\loom-desktop.exe')) -and (Test-Path (Join-Path $unpack '*\loom.exe'))
  } catch {
    $with_desktop = $false
  }
  Remove-Item "$unpack.zip" -Force -ErrorAction SilentlyContinue
}
if ($with_desktop) {
  Move-Item -Force (Join-Path $unpack '*\loom.exe') "$exe.new"
  Move-Item -Force (Join-Path $unpack '*\loom-desktop.exe') "$app.new"
} else {
  Write-Output '이 릴리스에는 데스크톱 앱이 없습니다. 브라우저 판 loom만 설치합니다.'
  Invoke-WebRequest -Uri $url -OutFile "$exe.new" -UseBasicParsing
}
Remove-Item $unpack -Recurse -Force -ErrorAction SilentlyContinue

if ($with_linux) { Move-Item -Force "$linux.new" $linux }
Install-File "$exe.new" $exe
if ($with_desktop) { Install-File "$app.new" $app }

$icon = Join-Path $home_dir 'icon.ico'
try { Invoke-WebRequest -Uri "$raw/app/icons/icon.ico" -OutFile $icon -UseBasicParsing } catch { $icon = $null }

$desktop = if ($env:LOOM_DESKTOP) { $env:LOOM_DESKTOP } else { [Environment]::GetFolderPath('Desktop') }
$shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $desktop 'loom.lnk'))
if ($with_desktop) {
  $shortcut.TargetPath = $app
  $shortcut.Arguments = ''
  $shortcut.WindowStyle = 1
} else {
  $shortcut.TargetPath = $exe
  $shortcut.Arguments = 'open'
  $shortcut.WindowStyle = 7
}
$shortcut.Description = 'loom'
if ($icon) { $shortcut.IconLocation = $icon }
$shortcut.Save()
Write-Output "바탕 화면에 loom 아이콘을 만들었습니다: $(Join-Path $desktop 'loom.lnk')"

if ($with_desktop) { Start-Process $app } else { & $exe open }
