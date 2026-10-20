// 데몬이 보낸 코드(gen/Problem·UsageProblem)를 현재 언어 문장으로 바꾼다.
// 키는 코드 이름이라 Rust에 변형을 더하고 번역을 빠뜨리면 `tsc`가 실패한다.
import type { TOptions } from 'i18next';
import i18next from './i18n';
import type { Problem } from './gen/Problem';
import type { Refusal } from './gen/Refusal';
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
    case 'repo':
      return i18next.t(`problem.repo.${p.value.code}`, vars(p.value));
    case 'noLogin':
      return i18next.t('problem.noLogin');
    case 'unsupportedImage':
      return i18next.t('problem.unsupportedImage', { value: p.value });
    case 'internal':
      return i18next.t('problem.internal', { value: p.value });
  }
}

/** 받는 노드 이름을 뺀 거절 문장. 이벤트 줄처럼 받는 노드를 따로 보일 때 쓴다. `_nameless` 키는 받는 이름을 품는 보내기 거절에만 둔다. 키가 없는 코드는 i18next가 원래 문장으로 돌아간다. */
export function refusalWithoutName(r: Refusal): string {
  const options: TOptions = { ...vars(r), context: 'nameless' };
  return i18next.t(`problem.refused.${r.code}`, options);
}

export function usageProblemText(p: UsageProblem): string {
  return i18next.t(`usageProblem.${p.code}`, vars(p));
}
