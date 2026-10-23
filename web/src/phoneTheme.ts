import { useState } from 'react';
import type { Theme } from './gen/Theme';
import type { ColorScheme } from './uiColors';

const KEY = 'loom.phoneTheme';
const THEMES: readonly Theme[] = ['system', 'light', 'dark'];

function read(): Theme | null {
  try {
    const stored = localStorage.getItem(KEY);
    return THEMES.find((theme) => theme === stored) ?? null;
  } catch {
    return null;
  }
}

export function usePhoneTheme() {
  const [choice, setChoice] = useState(read);
  const choose = (next: Theme) => {
    setChoice(next);
    try {
      localStorage.setItem(KEY, next);
    } catch {
    }
  };
  return [choice, choose] as const;
}

export function syncAppChrome(scheme: ColorScheme) {
  (window as { loomChrome?: { setScheme: (scheme: string) => void } }).loomChrome?.setScheme(scheme);
}
