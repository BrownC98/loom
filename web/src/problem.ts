import type { TOptions } from 'i18next';
import i18next from './i18n';
import type { Problem } from './gen/Problem';
import type { Refusal } from './gen/Refusal';
import type { UsageProblem } from './gen/UsageProblem';

function vars(coded: { code: string }): Record<string, unknown> {
  const value: unknown = 'value' in coded ? coded.value : undefined;
  const joined = (v: unknown) => (Array.isArray(v) ? v.join(', ') : v);
  if (Array.isArray(value) || typeof value !== 'object' || value === null) return { value: joined(value) };
  return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, joined(v)]));
}

export const statusText = (status: number) => i18next.t('problem.status', { status });

export function problemText(p: Problem): string {
  switch (p.code) {
    case 'refused':
      return i18next.t(`problem.refused.${p.value.code}`, vars(p.value));
    case 'launch': {
      const launch = p.value;
      if (launch.code === 'wsl') return i18next.t(`problem.wsl.${launch.value.code}`, { ...launch.value });
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

export function refusalWithoutName(r: Refusal): string {
  const options: TOptions = { ...vars(r), context: 'nameless' };
  return i18next.t(`problem.refused.${r.code}`, options);
}

export function usageProblemText(p: UsageProblem): string {
  return i18next.t(`usageProblem.${p.code}`, vars(p));
}
