; Tauri NSIS 설치 프로그램의 훅. `app/tauri.conf.json`의 `bundle.windows.nsis.installerHooks`가 가리킨다.

; 데몬과 노드의 PTY 호스트는 앱이 끝난 뒤에도 옛 loom.exe로 돈다. 새 앱의 `loom start`가 옛 판 데몬을 바꾼다. 도는 실행 파일은 덮어 쓰지도 지우지도 못하지만
; 이름은 바꿀 수 있다(실측, docs/design.md 「데스크톱 앱」). 그래서 옛 파일의 이름을 바꿔 두고 새 파일을 쓴다.
; 도는 노드가 없으면 그냥 지운다. 바꾼 파일은 그 노드가 모두 끝난 뒤의 다음 설치가 지운다.
; 깔린 loom.exe가 이 설치보다 새 판이면(`scripts/host.sh` 개발 빌드) `.keep`으로 비켜 두고, 설치 뒤 훅이 되돌린다.
; 근거: 2026-10-22 앱 업데이트(0.1.716)가 도는 개발 빌드(0.1.899)를 옛 판으로 바꿨다. 「새 판이 이긴다」(docs/design.md 「데몬 하나」).
; 끊긴 설치가 남긴 `.keep`은 먼저 정리한다: `loom.exe`와 비교해 새 판을 `loom.exe`로 남긴다.

; 지우고, 도는 파일이라 지우지 못하면 이름을 `.old`로 바꾼다.
!macro LOOM_DISCARD FILE
  Delete "${FILE}"
  ${If} ${FileExists} "${FILE}"
    System::Call "kernel32::GetTickCount64() l .r0"
    Rename "${FILE}" "${FILE}.$0.old"
  ${EndIf}
!macroend

; `loom --version`의 판(`loom 0.1.899` → `0.1.899`). 파일이 없거나 실패하면 빈 값이다.
!macro LOOM_VERSION FILE OUT
  nsExec::ExecToStack '"${FILE}" --version'
  Pop $0
  Pop ${OUT}
  ${WordFind} "${OUT}" " " "+2" ${OUT}
  ${If} $0 != 0
    StrCpy ${OUT} ""
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  Delete "$INSTDIR\loom.exe.*.old"
  ${If} ${FileExists} "$INSTDIR\loom.exe.keep"
    !insertmacro LOOM_VERSION "$INSTDIR\loom.exe" $1
    !insertmacro LOOM_VERSION "$INSTDIR\loom.exe.keep" $2
    ${VersionCompare} "$2" "$1" $0
    ${If} $0 == 1
      !insertmacro LOOM_DISCARD "$INSTDIR\loom.exe"
      Rename "$INSTDIR\loom.exe.keep" "$INSTDIR\loom.exe"
    ${Else}
      !insertmacro LOOM_DISCARD "$INSTDIR\loom.exe.keep"
    ${EndIf}
  ${EndIf}
  !insertmacro LOOM_VERSION "$INSTDIR\loom.exe" $1
  ${VersionCompare} "$1" "${VERSION}" $0
  ${If} $0 == 1
    Rename "$INSTDIR\loom.exe" "$INSTDIR\loom.exe.keep"
  ${EndIf}
  !insertmacro LOOM_DISCARD "$INSTDIR\loom.exe"
  ClearErrors
!macroend

; Tauri 템플릿은 설치 화면에 머문다(MUI_FINISHPAGE_NOAUTOCLOSE). 설치가 끝나면 마지막 화면으로 바로 넘긴다.
; 근거: 사람 지시 2026-10-20 「설치 완료시 바로 설치 프로그램 마지막 화면으로 이동하게 해줘」.
; 시작 메뉴에 「loom (browser)」를 더한다: 앱 창 대신 기본 브라우저로 같은 데몬의 화면을 연다(`loom open`, docs/design.md 「배포」).
; `loom open`은 콘솔 프로그램이라 창을 최소화해 띄운다. 브라우저를 연 뒤 곧 끝난다. 업데이트도 다시 만든다.
!macro NSIS_HOOK_POSTINSTALL
  SetAutoClose true
  ${If} ${FileExists} "$INSTDIR\loom.exe.keep"
    Delete "$INSTDIR\loom.exe"
    Rename "$INSTDIR\loom.exe.keep" "$INSTDIR\loom.exe"
  ${EndIf}
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
