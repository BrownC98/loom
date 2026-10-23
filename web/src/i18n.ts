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
let clock: Intl.DateTimeFormat;
let dayTime: Intl.DateTimeFormat;
let monthDayFormat: Intl.DateTimeFormat;
let units: Record<'minute' | 'hour' | 'day', Intl.NumberFormat>;
const unit = (locale: string, unit: 'minute' | 'hour' | 'day') => new Intl.NumberFormat(locale, { style: 'unit', unit, unitDisplay: 'narrow' });
// `init`도 이 이벤트를 낸다.
i18next.on('languageChanged', (locale) => {
  document.documentElement.lang = locale;
  relative = new Intl.RelativeTimeFormat(locale, { numeric: 'auto' });
  weekdayTime = new Intl.DateTimeFormat(locale, { weekday: 'short', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' });
  clock = new Intl.DateTimeFormat(locale, { hour: '2-digit', minute: '2-digit', second: '2-digit', hourCycle: 'h23' });
  dayTime = new Intl.DateTimeFormat(locale, { month: 'numeric', day: 'numeric', hour: 'numeric', minute: '2-digit' });
  monthDayFormat = new Intl.DateTimeFormat(locale, { month: 'short', day: 'numeric' });
  units = { minute: unit(locale, 'minute'), hour: unit(locale, 'hour'), day: unit(locale, 'day') };
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

/**
 * 남은 시간을 같은 언어의 좁은 단위로 쓴다. 예: 「47분」, 「3시간 12분」, 「4일 3시간」, 「47m」, 「3h 12m」.
 * 1분이 안 되는 값은 0으로 적는다. 부르는 쪽이 「곧」으로 바꾼다.
 */
export function duration(ms: number): string {
  const minutes = Math.max(0, Math.floor(ms / 60000));
  if (minutes < 60) return units.minute.format(minutes);
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return joined(hours, minutes % 60, 'hour', 'minute');
  return joined(Math.floor(hours / 24), hours % 24, 'day', 'hour');
}

/** 작은 값이 0이면 빼고 쓴다. 예: 「3시간」, 「3시간 12분」. */
function joined(big: number, small: number, bigUnit: 'hour' | 'day', smallUnit: 'minute' | 'hour'): string {
  const bigText = units[bigUnit].format(big);
  return small === 0 ? bigText : `${bigText} ${units[smallUnit].format(small)}`;
}

/** 예: 「2026/10/22 15:40:07」. 언어와 관계없이 이 꼴이다(2026-10-22 사람 지시 "년도/월/일 시:분:초 포맷으로 표시"). */
export function dateAndTime(time: number): string {
  const d = new Date(time);
  const two = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}/${two(d.getMonth() + 1)}/${two(d.getDate())} ${two(d.getHours())}:${two(d.getMinutes())}:${two(d.getSeconds())}`;
}

/** 예: 「(월) 15:40」, 「Mon 15:40」. */
export function weekdayAndTime(time: number): string {
  return weekdayTime.format(time);
}

/** 예: 「11월 20일」, 「Nov 20」. Orca가 Cursor 결제 주기 끝을 이렇게 쓴다. */
export function monthDay(time: number): string {
  return monthDayFormat.format(time);
}

/** 예: 「10. 23. 오후 3:40」, 「10/23, 3:40 PM」. 해는 적지 않는다. */
export function dayAndTime(time: number): string {
  return dayTime.format(time);
}

/** 예: 「15:40:07」. */
export function clockTime(time: number): string {
  return clock.format(time);
}

export default i18next;
