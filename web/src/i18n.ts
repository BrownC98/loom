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

function resolve(language: Language): Locale {
  if (language !== 'system') return language;
  return navigator.language.toLowerCase().startsWith('ko') ? 'ko' : 'en';
}

export const INITIAL_LANGUAGE = (document.documentElement.dataset.language as Language | undefined) ?? 'system';

let relative: Intl.RelativeTimeFormat;
let weekdayTime: Intl.DateTimeFormat;
let clock: Intl.DateTimeFormat;
let dayTime: Intl.DateTimeFormat;
let monthDayFormat: Intl.DateTimeFormat;
let units: Record<'minute' | 'hour' | 'day', Intl.NumberFormat>;
const unit = (locale: string, unit: 'minute' | 'hour' | 'day') => new Intl.NumberFormat(locale, { style: 'unit', unit, unitDisplay: 'narrow' });
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

export function ago(time: number, now: number): string {
  const ms = time - now;
  for (const [unit, size] of STEPS) {
    if (Math.abs(ms) >= size) return relative.format(Math.round(ms / size), unit);
  }
  return relative.format(0, 'second');
}

export function duration(ms: number): string {
  const minutes = Math.max(0, Math.floor(ms / 60000));
  if (minutes < 60) return units.minute.format(minutes);
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return joined(hours, minutes % 60, 'hour', 'minute');
  return joined(Math.floor(hours / 24), hours % 24, 'day', 'hour');
}

function joined(big: number, small: number, bigUnit: 'hour' | 'day', smallUnit: 'minute' | 'hour'): string {
  const bigText = units[bigUnit].format(big);
  return small === 0 ? bigText : `${bigText} ${units[smallUnit].format(small)}`;
}

export function dateAndTime(time: number): string {
  const d = new Date(time);
  const two = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}/${two(d.getMonth() + 1)}/${two(d.getDate())} ${two(d.getHours())}:${two(d.getMinutes())}:${two(d.getSeconds())}`;
}

export function weekdayAndTime(time: number): string {
  return weekdayTime.format(time);
}

export function monthDay(time: number): string {
  return monthDayFormat.format(time);
}

export function dayAndTime(time: number): string {
  return dayTime.format(time);
}

export function clockTime(time: number): string {
  return clock.format(time);
}

export default i18next;
