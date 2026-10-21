// 계정과 사용량을 보이는 공용 부품. 상태 줄, 설정 창, 새 노드 창이 같은 문구와 색을 쓴다.
import { useEffect, useState } from 'react';
import type { AccountId } from './gen/AccountId';
import type { AccountView } from './gen/AccountView';
import type { Limits } from './gen/Limits';
import type { Provider } from './gen/Provider';
import type { SettingsView } from './gen/SettingsView';
import type { Window } from './gen/Window';
import i18next, { ago, duration, weekdayAndTime } from './i18n';
import { usageProblemText } from './problem';

/** 이 비율 이상 쓰면 막대가 노란색이다(목업 usage.html). */
const HIGH_USED = 75;

export const PROVIDERS: Provider[] = ['claude', 'codex'];
export const PROVIDER_LABEL: Record<Provider, string> = { claude: 'Claude', codex: 'Codex' };
/** 시스템 기본 계정의 로그인 폴더. loom은 사람이 [로그인]·[로그아웃]을 누를 때만 CLI로 바꾼다. */
export const SYSTEM_DIR: Record<Provider, string> = { claude: '~/.claude', codex: '~/.codex' };

/** 새 노드가 쓰는 계정. 설정이 고른 계정이 없거나 지워졌으면 시스템 기본이다. */
export function activeAccount(accounts: AccountView[], settings: SettingsView, provider: Provider): AccountView | undefined {
  const chosen = provider === 'claude' ? settings.claudeAccount : settings.codexAccount;
  const own = accounts.filter((a) => a.provider === provider);
  return own.find((a) => a.id !== null && a.id === chosen) ?? own.find((a) => a.id === null);
}

/** 공급자의 쓰는 계정을 바꾼 설정. `null`이면 시스템 기본이다. */
export function withAccount(settings: SettingsView, provider: Provider, id: AccountId | null): SettingsView {
  return provider === 'claude' ? { ...settings, claudeAccount: id } : { ...settings, codexAccount: id };
}

/** 이메일을 알면 이메일이다. 시스템 기본은 「시스템 기본」 배지가 따로 붙는다. */
export function accountName(account: AccountView): string {
  return account.usage.email ?? i18next.t(account.id === null ? 'usage.systemDefault' : 'usage.notLoggedIn');
}

/** 사용량을 보일 수 없는 이유. 보일 수 있으면 null. */
export function limitsProblem(limits: Limits): string | null {
  switch (limits.kind) {
    case 'ready':
      return null;
    case 'needsLogin':
      return i18next.t('usage.needsLogin');
    case 'failed':
      return usageProblemText(limits.problem);
  }
}

export function Meter(props: { used: number }) {
  return (
    <span className="meter" aria-hidden="true">
      <i className={props.used >= HIGH_USED ? 'low' : undefined} style={{ width: `${props.used}%` }} />
    </span>
  );
}

type NamedWindow = { label: string; window: Window };

/** 공급자 전체에 걸린 창: 5시간, 주간. 사용량이 없으면 빈 목록이다. */
function accountWindows(limits: Limits): NamedWindow[] {
  if (limits.kind !== 'ready') return [];
  return (['session', 'weekly'] as const).flatMap((key) => {
    const w = limits[key];
    return w ? [{ label: i18next.t(`usage.window.${key}`), window: w }] : [];
  });
}

/** 보일 창 목록: 공급자 전체 창 뒤에 모델별 주간 창(예: Fable). */
function windows(limits: Limits): NamedWindow[] {
  const models = limits.kind === 'ready' ? limits.models.map((m) => ({ label: m.model, window: m.window })) : [];
  return [...accountWindows(limits), ...models];
}

/** 창마다 이름, 막대, 사용한 비율, 남은 시간 한 줄. 사용량이 없으면 아무것도 그리지 않는다. */
export function WindowLines(props: { limits: Limits }) {
  const now = useMinute();
  return windows(props.limits).map(({ label, window: w }) => {
    const left = remainingText(w.resetsAt, now);
    return (
      <span className="win-line" key={label}>
        <span>{label}</span>
        <Meter used={w.used} />
        <span className="pct">{w.used}%</span>
        <span className="left">{left}</span>
      </span>
    );
  });
}

/** 남은 시간이 1분마다 줄도록 다시 그린다. */
function useMinute(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 60 * 1000);
    return () => window.clearInterval(timer);
  }, []);
  return now;
}

/** 상태 줄 안의 짧은 표시: 막대와 「5시간 58%」. */
export function InlineWindows(props: { limits: Limits }) {
  return windows(props.limits).map(({ label, window: w }) => (
    <span className="lim" key={label}>
      <Meter used={w.used} />
      {label} {w.used}%
    </span>
  ));
}

/** 읽는 사람에게 들리는 한 줄. 예: 「5시간 42% 사용, 주간 18% 사용」. */
export function windowsText(limits: Limits): string {
  if (limits.kind !== 'ready') return limitsProblem(limits) ?? '';
  return windows(limits)
    .map(({ label, window: w }) => i18next.t('usage.windowUsed', { window: label, n: w.used }))
    .join(', ');
}

/** 예: 「3시간 12분 남음」. 초기화 시각이 없으면 빈 글자다. */
function remainingText(resetsAt: number | null, now: number): string {
  const ms = resetsAt === null ? null : resetsAt * 1000 - now;
  if (ms === null) return '';
  return ms < 60 * 1000 ? i18next.t('usage.soon') : i18next.t('usage.remaining', { left: duration(ms) });
}

/** 예: 「5시간 창 초기화 (월) 15:40 · 주간 창 초기화 (목) 09:00」. 모델 창은 주간 창과 함께 초기화되므로 넣지 않는다(목업 usage.html). */
export function resetText(limits: Limits): string {
  return accountWindows(limits)
    .flatMap(({ label, window: w }) => (w.resetsAt === null ? [] : [i18next.t('usage.reset', { window: label, time: weekdayAndTime(w.resetsAt * 1000) })]))
    .join(' · ');
}

/** 예: 「2분 전 갱신」. */
export function fetchedText(at: number, now: number): string {
  return i18next.t('usage.fetched', { ago: ago(at, now) });
}
