; Tauri NSIS 설치 프로그램의 훅. `app/tauri.conf.json`의 `bundle.windows.nsis.installerHooks`가 가리킨다.

; 데몬과 노드의 PTY 호스트는 앱이 끝난 뒤에도 옛 loom.exe로 돈다. 새 앱의 `loom start`가 옛 판 데몬을 바꾼다. 도는 실행 파일은 덮어 쓰지도 지우지도 못하지만
; 이름은 바꿀 수 있다(실측, docs/design.md 「데스크톱 앱」). 그래서 옛 파일의 이름을 바꿔 두고 새 파일을 쓴다.
; 도는 노드가 없으면 그냥 지운다. 바꾼 파일은 그 노드가 모두 끝난 뒤의 다음 설치가 지운다.
!macro NSIS_HOOK_PREINSTALL
  Delete "$INSTDIR\loom.exe.*.old"
  Delete "$INSTDIR\loom.exe"
  ${If} ${FileExists} "$INSTDIR\loom.exe"
    System::Call "kernel32::GetTickCount64() l .r0"
    Rename "$INSTDIR\loom.exe" "$INSTDIR\loom.exe.$0.old"
  ${EndIf}
  ClearErrors
!macroend

; Tauri 템플릿은 설치 화면에 머문다(MUI_FINISHPAGE_NOAUTOCLOSE). 설치가 끝나면 마지막 화면으로 바로 넘긴다.
; 근거: 사람 지시 2026-10-20 「설치 완료시 바로 설치 프로그램 마지막 화면으로 이동하게 해줘」.
; 시작 메뉴에 「loom (browser)」를 더한다: 앱 창 대신 기본 브라우저로 같은 데몬의 화면을 연다(`loom open`, docs/design.md 「배포」).
; `loom open`은 콘솔 프로그램이라 창을 최소화해 띄운다. 브라우저를 연 뒤 곧 끝난다. 업데이트도 다시 만든다.
!macro NSIS_HOOK_POSTINSTALL
  SetAutoClose true
  CreateShortcut "$SMPROGRAMS\loom (browser).lnk" "$INSTDIR\loom.exe" "open" "$INSTDIR\${MAINBINARYNAME}.exe" 0 SW_SHOWMINIMIZED
!macroend

; 지우기는 노드를 남기지 않는다. 이 폴더의 loom.exe(이름을 바꾼 옛 파일 포함)로 도는 데몬·PTY 호스트와 그 자손을 끝내고,
; 옛 파일과 「loom (browser)」 바로 가기를 지운다. 이름으로 고르지 않는다. 다른 폴더의 loom.exe는 이 설치의 것이 아니다.
; 업데이트는 지우기를 부르지 않는다(이전 설치 확인을 `Abort`로 바꾼다, `.github/workflows/release.yml`).
; 프로세스 경로는 긴 이름이다. `$INSTDIR`이 짧은 이름(8.3)일 수 있어 긴 이름으로 바꿔 비교한다(실측).
; 이름을 바꾼 파일로 도는 프로세스도 경로는 처음 띄운 이름(`loom.exe`)으로 보인다(실측).
!macro NSIS_HOOK_PREUNINSTALL
  System::Call "kernel32::GetLongPathNameW(w '$INSTDIR', w .r0, i ${NSIS_MAX_STRLEN})"
  nsExec::Exec `powershell.exe -NoProfile -NonInteractive -Command "Get-CimInstance Win32_Process | Where-Object { $$_.ExecutablePath -eq '$0\loom.exe' } | ForEach-Object { taskkill.exe /F /T /PID $$_.ProcessId }"`
  Pop $0
  Sleep 1000
  Delete "$INSTDIR\loom.exe.*.old"
  Delete "$SMPROGRAMS\loom (browser).lnk"
!macroend
