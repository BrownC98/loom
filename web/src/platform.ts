import type { KeyboardEvent as ReactKeyboardEvent } from 'react';

export const MAC = navigator.userAgent.includes('Mac');

export function commandKey(e: KeyboardEvent | ReactKeyboardEvent): boolean {
  return MAC ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
}

export function shortcut(key: string): { label: string; aria: string } {
  return MAC ? { label: `⌘⇧${key}`, aria: `Meta+Shift+${key}` } : { label: `Ctrl+Shift+${key}`, aria: `Control+Shift+${key}` };
}
