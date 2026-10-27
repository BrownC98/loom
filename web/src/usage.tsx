import { useEffect, useState } from 'react';
import type { AccountId } from './gen/AccountId';
import type { AccountView } from './gen/AccountView';
import type { PlaceUsage } from './gen/PlaceUsage';
import type { Limits } from './gen/Limits';
import type { Provider } from './gen/Provider';
import type { SettingsView } from './gen/SettingsView';
import type { Window } from './gen/Window';
import i18next, { ago, duration, monthDay, weekdayAndTime } from './i18n';
import { usageProblemText } from './problem';

const HIGH_USED = 75;

export const PROVIDERS: Provider[] = ['claude', 'codex'];
export const PROVIDER_LABEL: Record<Provider, string> = { claude: 'Claude', codex: 'Codex' };
export const SYSTEM_DIR: Record<Provider, string> = { claude: '~/.claude', codex: '~/.codex' };

export function placesOf(accounts: AccountView[]): (string | null)[] {
  return [...new Set(accounts.map((a) => a.distro))];
}

export function activeAccount(accounts: AccountView[], settings: SettingsView, provider: Provider, distro: string | null): AccountView | undefined {
  const own = accounts.filter((a) => a.provider === provider && a.distro === distro);
  const chosen = chosenAccount(settings, provider, distro);
  return own.find((a) => a.id !== null && a.id === chosen) ?? own.find((a) => a.id === null);
}

function chosenAccount(settings: SettingsView, provider: Provider, distro: string | null): AccountId | null {
  return (distro === null ? settings.accounts : settings.wslAccounts[distro])?.[provider] ?? null;
}

export function withAccount(settings: SettingsView, provider: Provider, distro: string | null, id: AccountId | null): SettingsView {
  const inUse = { ...(distro === null ? settings.accounts : settings.wslAccounts[distro]), [provider]: id ?? undefined };
  return distro === null ? { ...settings, accounts: inUse } : { ...settings, wslAccounts: { ...settings.wslAccounts, [distro]: inUse } };
}

export function accountName(account: AccountView): string {
  return account.usage.email ?? i18next.t(account.id === null ? 'usage.systemDefault' : 'usage.notLoggedIn');
}

export function notInstalled(a: AccountView): boolean {
  return cliMissing(a.usage.limits);
}

export function cliMissing(limits: Limits): boolean {
  return limits.kind === 'failed' && limits.problem.code === 'notInstalled';
}

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

function accountWindows(limits: Limits): NamedWindow[] {
  if (limits.kind !== 'ready') return [];
  return (['session', 'weekly'] as const).flatMap((key) => {
    const w = limits[key];
    return w ? [{ label: i18next.t(`usage.window.${key}`), window: w }] : [];
  });
}

function inlineWindows(limits: Limits): NamedWindow[] {
  const models = limits.kind === 'ready' ? limits.models.map((m) => ({ label: m.model, window: m.window })) : [];
  return [...accountWindows(limits), ...models];
}

function windows(limits: Limits): NamedWindow[] {
  const plan = limits.kind === 'ready' && limits.plan ? [{ label: i18next.t('usage.window.plan'), window: limits.plan }] : [];
  return [...inlineWindows(limits), ...plan];
}

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

function useMinute(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 60 * 1000);
    return () => window.clearInterval(timer);
  }, []);
  return now;
}

export function InlineWindows(props: { limits: Limits }) {
  return inlineWindows(props.limits).map(({ label, window: w }) => (
    <span className="lim" key={label}>
      <Meter used={w.used} />
      {label} {w.used}%
    </span>
  ));
}

export function windowsText(limits: Limits): string {
  if (limits.kind !== 'ready') return limitsProblem(limits) ?? '';
  return windows(limits)
    .map(({ label, window: w }) => i18next.t('usage.windowUsed', { window: label, n: w.used }))
    .join(', ');
}

function remainingText(resetsAt: number | null, now: number): string {
  const ms = resetsAt === null ? null : resetsAt * 1000 - now;
  if (ms === null) return '';
  return ms < 60 * 1000 ? i18next.t('usage.soon') : i18next.t('usage.remaining', { left: duration(ms) });
}

export function resetText(limits: Limits): string {
  const windows = accountWindows(limits).flatMap(({ label, window: w }) =>
    w.resetsAt === null ? [] : [i18next.t('usage.reset', { window: label, time: weekdayAndTime(w.resetsAt * 1000) })],
  );
  const planReset = limits.kind === 'ready' ? limits.plan?.resetsAt : null;
  const plan = planReset == null ? [] : [i18next.t('usage.resetsOn', { date: monthDay(planReset * 1000) })];
  return [...windows, ...plan].join(' · ');
}

export function fetchedText(readAt: number, now: number): string {
  return i18next.t('usage.fetched', { ago: ago(readAt * 1000, now) });
}

export function oldestRead(accounts: AccountView[] | null, cursor: PlaceUsage[] | null): number | null {
  const times = [...(accounts ?? []).map((a) => a.usage.readAt), ...(cursor ?? []).map((c) => c.usage.readAt)];
  return times.length === 0 ? null : Math.min(...times);
}
