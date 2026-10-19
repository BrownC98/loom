// 화면 문구의 언어. 문구는 locales/에, 키 타입은 영어 파일이 원천이다. 날짜·시간은 `Intl`이 같은 언어로 쓴다.
import i18next from 'i18next';
import { initReactI18next } from 'react-i18next';
import type { Language } from './gen/Language';
import en from './locales/en';
import ko from './locales/ko';

declare module 'i18next' {
  interface CustomTypeOptions {
    resources: { translation: typeof en };
  }
}

type Locale = Exclude<Language, 'system'>;

/** 「시스템」이면 브라우저 언어가 한국어일 때만 한국어다. Orca와 같은 규칙이다. */
function resolve(language: Language): Locale {
  if (language !== 'system') return language;
  return navigator.language.toLowerCase().startsWith('ko') ? 'ko' : 'en';
}

/** 데몬이 첫 페이지에 넣은 언어(`Language`의 serde 이름)다. 설정을 받기 전에도 같은 언어로 그린다. */
export const INITIAL_LANGUAGE = (document.documentElement.dataset.language as Language | undefined) ?? 'system';

let relative: Intl.RelativeTimeFormat;
let weekdayTime: Intl.DateTimeFormat;
// `init`도 이 이벤트를 낸다.
i18next.on('languageChanged', (locale) => {
  document.documentElement.lang = locale;
  relative = new Intl.RelativeTimeFormat(locale, { numeric: 'auto' });
  weekdayTime = new Intl.DateTimeFormat(locale, { weekday: 'short', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' });
});
i18next.use(initReactI18next).init({
  resources: { en: { translation: en }, ko: { translation: ko } },
  lng: resolve(INITIAL_LANGUAGE),
  fallbackLng: 'en',
  interpolation: { escapeValue: false },
});

/** 설정의 언어로 바꾼다. `useTranslation`을 쓰는 부품이 다시 그린다. */
export function applyLanguage(language: Language) {
  i18next.changeLanguage(resolve(language));
}

const STEPS: [Intl.RelativeTimeFormatUnit, number][] = [
  ['year', 365 * 24 * 3600 * 1000],
  ['month', 30 * 24 * 3600 * 1000],
  ['week', 7 * 24 * 3600 * 1000],
  ['day', 24 * 3600 * 1000],
  ['hour', 3600 * 1000],
  ['minute', 60 * 1000],
];

/** `time`(ms)이 `now`(ms)에서 얼마 전인지. 예: 「3분 전」. 1분이 안 되면 「지금」. */
export function ago(time: number, now: number): string {
  const ms = time - now;
  for (const [unit, size] of STEPS) {
    if (Math.abs(ms) >= size) return relative.format(Math.round(ms / size), unit);
  }
  return relative.format(0, 'second');
}

/** 예: 「(월) 15:40」, 「Mon 15:40」. */
export function weekdayAndTime(time: number): string {
  return weekdayTime.format(time);
}

export default i18next;
