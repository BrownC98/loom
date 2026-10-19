// 화면 문구의 언어. 문구는 locales/에, 키 타입은 영어 파일이 원천이다.
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

i18next.use(initReactI18next).init({
  resources: { en: { translation: en }, ko: { translation: ko } },
  lng: resolve(INITIAL_LANGUAGE),
  fallbackLng: 'en',
  interpolation: { escapeValue: false },
});
document.documentElement.lang = i18next.language;

/** 설정의 언어로 바꾼다. `useTranslation`을 쓰는 부품이 다시 그린다. */
export function applyLanguage(language: Language) {
  const locale = resolve(language);
  document.documentElement.lang = locale;
  i18next.changeLanguage(locale);
}

export default i18next;
