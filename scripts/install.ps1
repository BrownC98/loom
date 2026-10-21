# Windows에 loom 웹 버전(앱 없이 브라우저로)을 설치하고 바탕 화면에 loom 아이콘을 만든다. 아이콘을 누르면 `loom open`이 데몬을 띄우고 브라우저를 연다.
# 사람은 PowerShell에서 한 줄로 부른다: irm https://raw.githubusercontent.com/BrownC98/loom/main/scripts/install.ps1 | iex
# 다시 부르면 업데이트다: 파일을 바꿔 넣고 옛 판 데몬만 새 판으로 바꾼다. 노드는 그대로 산다. 그 뒤 업데이트는 화면의 [업데이트]로도 한다.
# 근거: docs/design.md 「배포」. Linux·WSL·macOS는 `install.sh`다.
# 시험용 변수: LOOM_DOWNLOAD_URL(실행 파일 주소), LOOM_DESKTOP(바탕 화면 폴더).
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$url = if ($env:LOOM_DOWNLOAD_URL) { $env:LOOM_DOWNLOAD_URL } else { 'https://github.com/BrownC98/loom/releases/latest/download/loom-x86_64-windows.exe' }
$raw = 'https://raw.githubusercontent.com/BrownC98/loom/main'
# 노드 안의 에이전트가 `loom` 명령을 부른다. 그래서 이름은 `loom.exe`다.
$home_dir = Join-Path $env:LOCALAPPDATA 'loom'
$dir = Join-Path $home_dir 'bin'
$exe = Join-Path $dir 'loom.exe'
New-Item -ItemType Directory -Force -Path $dir | Out-Null

# 새 파일을 받은 뒤 바꿔 넣는다. 도는 loom.exe(데몬, 노드의 PTY 호스트)는 덮어 쓰지도 지우지도 못하지만 이름은 바꿀 수 있다.
# 이름 꼴은 앱 설치 파일과 데몬 업데이트(`src/update.rs`)와 같다. 앞에서 바꿔 둔 옛 파일은 지금 지운다. 아직 쓰는 파일은 남는다.
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

# 아이콘을 받지 못하면 기본 아이콘으로 둔다.
$icon = Join-Path $home_dir 'icon.ico'
try { Invoke-WebRequest -Uri "$raw/app/icons/icon.ico" -OutFile $icon -UseBasicParsing } catch { $icon = $null }

# 바로 가기는 최소화한 창(WindowStyle 7)으로 연다. `loom open`은 브라우저를 열고 곧 끝나므로 창이 남지 않는다.
# OneDrive를 쓰면 바탕 화면이 OneDrive 안에 있으므로 Windows에 묻는다.
$desktop = if ($env:LOOM_DESKTOP) { $env:LOOM_DESKTOP } else { [Environment]::GetFolderPath('Desktop') }
$shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $desktop 'loom.lnk'))
$shortcut.TargetPath = $exe
$shortcut.Arguments = 'open'
$shortcut.WindowStyle = 7
$shortcut.Description = 'loom'
if ($icon) { $shortcut.IconLocation = $icon }
$shortcut.Save()
Write-Output "바탕 화면에 loom 아이콘을 만들었습니다: $(Join-Path $desktop 'loom.lnk')"

# 도는 데몬이 옛 판이면 `open`이 새 판으로 바꾼다. 같은 판이나 새 판이면 붙는다. `daemon.json`의 데몬만 바꾼다(docs/design.md 「데몬 하나」).
& $exe open
