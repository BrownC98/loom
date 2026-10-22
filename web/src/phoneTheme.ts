// 이 폰만의 테마 고르기(Q464 A). PC 설정 `settings.theme`은 바꾸지 않는다.
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

/** 이 폰에서 고른 테마. 고른 적이 없으면 `null`이고 PC 설정을 따른다. */
export function usePhoneTheme() {
  const [choice, setChoice] = useState(read);
  const choose = (next: Theme) => {
    setChoice(next);
    try {
      localStorage.setItem(KEY, next);
    } catch {
      // 저장하지 못해도 이 세션에서는 바뀐다.
    }
  };
  return [choice, choose] as const;
}

/** 안드로이드 앱의 상태 줄 글자색과 바탕을 화면 테마에 맞춘다(`MainActivity`의 `loomChrome`). 옛 APK에는 이 다리가 없다. */
export function syncAppChrome(scheme: ColorScheme) {
  (window as { loomChrome?: { setScheme: (scheme: string) => void } }).loomChrome?.setScheme(scheme);
}
