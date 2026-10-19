// 데몬이 보낸 코드(gen/Problem·UsageProblem)를 현재 언어 문장으로 바꾼다.
// 키는 코드 이름이라 Rust에 변형을 더하고 번역을 빠뜨리면 `tsc`가 실패한다.
import i18next from './i18n';
import type { Problem } from './gen/Problem';
import type { UsageProblem } from './gen/UsageProblem';

/** 값이 객체면 그 필드를, 아니면 `{{value}}` 하나를 보간 값으로 쓴다. */
function vars(coded: { code: string }): Record<string, unknown> {
  const value: unknown = 'value' in coded ? coded.value : undefined;
  return typeof value === 'object' && value !== null ? { ...value } : { value };
}

export function problemText(p: Problem): string {
  switch (p.code) {
    case 'refused':
      return i18next.t(`problem.refused.${p.value.code}`, vars(p.value));
    case 'launch':
      return i18next.t(`problem.launch.${p.value.code}`, vars(p.value));
    case 'noLogin':
      return i18next.t('problem.noLogin');
    case 'internal':
      return i18next.t('problem.internal', { value: p.value });
  }
}

export function usageProblemText(p: UsageProblem): string {
  return i18next.t(`usageProblem.${p.code}`, vars(p));
}
