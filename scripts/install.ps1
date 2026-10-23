$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$url = if ($env:LOOM_DOWNLOAD_URL) { $env:LOOM_DOWNLOAD_URL } else { 'https://github.com/BrownC98/loom/releases/latest/download/loom-web-windows-x64.exe' }
$raw = 'https://raw.githubusercontent.com/BrownC98/loom/main'
$home_dir = Join-Path $env:LOCALAPPDATA 'loom'
$dir = Join-Path $home_dir 'bin'
$exe = Join-Path $dir 'loom.exe'
New-Item -ItemType Directory -Force -Path $dir | Out-Null

Get-ChildItem -Path $dir -Filter 'loom.exe.*.old' -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
Invoke-WebRequest -Uri $url -OutFile "$exe.new" -UseBasicParsing
if (Test-Path $exe) {
  try {
    Remove-Item $exe -Force
  } catch {
    Rename-Item $exe "loom.exe.$([DateTimeOffset]::UtcNow.ToUnixTimeSeconds()).old"
  }
}
Move-Item "$exe.new" $exe

$icon = Join-Path $home_dir 'icon.ico'
try { Invoke-WebRequest -Uri "$raw/app/icons/icon.ico" -OutFile $icon -UseBasicParsing } catch { $icon = $null }

$desktop = if ($env:LOOM_DESKTOP) { $env:LOOM_DESKTOP } else { [Environment]::GetFolderPath('Desktop') }
$shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $desktop 'loom.lnk'))
$shortcut.TargetPath = $exe
$shortcut.Arguments = 'open'
$shortcut.WindowStyle = 7
$shortcut.Description = 'loom'
if ($icon) { $shortcut.IconLocation = $icon }
$shortcut.Save()
Write-Output "바탕 화면에 loom 아이콘을 만들었습니다: $(Join-Path $desktop 'loom.lnk')"

& $exe open
