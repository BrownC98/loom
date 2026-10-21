// 화면의 OS 차이는 이 파일 한 곳에 둔다(사람 결정 2026-10-21 "맥에선 맥의 법칙을 따르라"). 근거: docs/design.md 「데스크톱 앱」.
import type { KeyboardEvent as ReactKeyboardEvent } from 'react';

/** macOS다. 앱(WKWebView)과 브라우저가 모두 사용자 에이전트에 `Mac`을 적는다. */
export const MAC = navigator.userAgent.includes('Mac');

/** 명령 키(macOS ⌘, 다른 OS Ctrl)만 눌렸다. Shift·Alt와 다른 쪽 키는 보지 않는다. 터미널의 Ctrl 글쇠는 macOS에서 터미널 앱에 간다. */
export function commandKey(e: KeyboardEvent | ReactKeyboardEvent): boolean {
  return MAC ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
}

/** 명령 키+Shift+`key` 단축키의 툴팁 글(`label`)과 `aria-keyshortcuts` 값(`aria`). */
export function shortcut(key: string): { label: string; aria: string } {
  return MAC ? { label: `⌘⇧${key}`, aria: `Meta+Shift+${key}` } : { label: `Ctrl+Shift+${key}`, aria: `Control+Shift+${key}` };
}
