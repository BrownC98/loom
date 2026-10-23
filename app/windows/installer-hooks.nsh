!macro LOOM_DISCARD FILE
  Delete "${FILE}"
  ${If} ${FileExists} "${FILE}"
    System::Call "kernel32::GetTickCount64() l .r0"
    Rename "${FILE}" "${FILE}.$0.old"
  ${EndIf}
!macroend

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

!macro NSIS_HOOK_POSTINSTALL
  SetAutoClose true
  ${If} ${FileExists} "$INSTDIR\loom.exe.keep"
    Delete "$INSTDIR\loom.exe"
    Rename "$INSTDIR\loom.exe.keep" "$INSTDIR\loom.exe"
  ${EndIf}
  CreateShortcut "$SMPROGRAMS\loom (browser).lnk" "$INSTDIR\loom.exe" "open" "$INSTDIR\${MAINBINARYNAME}.exe" 0 SW_SHOWMINIMIZED
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  System::Call "kernel32::GetLongPathNameW(w '$INSTDIR', w .r0, i ${NSIS_MAX_STRLEN})"
  nsExec::Exec `powershell.exe -NoProfile -NonInteractive -Command "Get-CimInstance Win32_Process | Where-Object { $$_.ExecutablePath -eq '$0\loom.exe' } | ForEach-Object { taskkill.exe /F /T /PID $$_.ProcessId }"`
  Pop $0
  Sleep 1000
  Delete "$INSTDIR\loom.exe.*.old"
  Delete "$SMPROGRAMS\loom (browser).lnk"
!macroend
