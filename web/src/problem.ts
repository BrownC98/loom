// 데몬이 보낸 코드(gen/Problem·UsageProblem)를 현재 언어 문장으로 바꾼다.
// 키는 코드 이름이라 Rust에 변형을 더하고 번역을 빠뜨리면 `tsc`가 실패한다.
import type { TOptions } from 'i18next';
import i18next from './i18n';
import type { Problem } from './gen/Problem';
import type { Refusal } from './gen/Refusal';
import type { UsageProblem } from './gen/UsageProblem';

/** 값이 목록이면 쉼표로 이은 `{{value}}`, 객체면 그 필드(목록 필드도 쉼표로 잇는다), 아니면 `{{value}}` 하나를 보간 값으로 쓴다. */
function vars(coded: { code: string }): Record<string, unknown> {
  const value: unknown = 'value' in coded ? coded.value : undefined;
  const joined = (v: unknown) => (Array.isArray(v) ? v.join(', ') : v);
  if (Array.isArray(value) || typeof value !== 'object' || value === null) return { value: joined(value) };
  return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, joined(v)]));
}

/** 본문이 Problem이 아닌 실패 응답의 문장. */
export const statusText = (status: number) => i18next.t('problem.status', { status });

export function problemText(p: Problem): string {
  switch (p.code) {
    case 'refused':
      return i18next.t(`problem.refused.${p.value.code}`, vars(p.value));
    case 'launch': {
      const launch = p.value;
      if (launch.code === 'wsl') return i18next.t(`problem.wsl.${launch.value.code}`, { ...launch.value });
      // WSL 노드의 에이전트가 그 배포판에 없으면 배포판 이름을 넣은 문장(`notInstalled_wsl`)이다.
      const context = launch.code === 'notInstalled' && launch.value.distro ? 'wsl' : undefined;
      return i18next.t(`problem.launch.${launch.code}`, { ...vars(launch), context });
    }
    case 'repo':
      return i18next.t(`problem.repo.${p.value.code}`, vars(p.value));
    case 'noLogin':
      return i18next.t('problem.noLogin');
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
