; Tauri NSIS 설치 프로그램의 훅. `app/tauri.conf.json`의 `bundle.windows.nsis.installerHooks`가 가리킨다.
; Tauri 템플릿은 설치 화면에 머문다(MUI_FINISHPAGE_NOAUTOCLOSE). 설치가 끝나면 마지막 화면으로 바로 넘긴다.
; 근거: 사람 지시 2026-10-20 「설치 완료시 바로 설치 프로그램 마지막 화면으로 이동하게 해줘」.
!macro NSIS_HOOK_POSTINSTALL
  SetAutoClose true
!macroend
