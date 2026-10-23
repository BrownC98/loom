import type { TFunction } from 'i18next';
import type { AccountId } from './gen/AccountId';
import type { Entry } from './gen/Entry';
import type { EventRecord } from './gen/EventRecord';
import type { Provider } from './gen/Provider';

type Translate = TFunction;

export type SwitchEntry = Extract<Entry, { kind: 'accountSwitched' | 'noAccountLeft' }>;

export function isSwitchEntry(entry: Entry): entry is SwitchEntry {
  return entry.kind === 'accountSwitched' || entry.kind === 'noAccountLeft';
}

export function latestSwitch(log: EventRecord[]): (EventRecord & { entry: SwitchEntry }) | null {
  return (log.find((r) => isSwitchEntry(r.entry)) as (EventRecord & { entry: SwitchEntry }) | undefined) ?? null;
}

export function isAutoSwitch(entry: Entry): boolean {
  return entry.kind === 'accountSwitched' && entry.why.kind !== 'manual';
}

export type Notice = { text: string; warn: boolean };

export function noticeOf(entry: SwitchEntry, name: (provider: Provider, id: AccountId | null) => string, time: (unixSeconds: number) => string, t: Translate): Notice {
  const provider = t(`switch.provider.${entry.provider}`);
  if (entry.kind === 'noAccountLeft') {
    const head = t('switch.notice.noneLeft', { provider });
    return { text: entry.resets_at === null ? head : `${head} · ${t('switch.notice.resetsAt', { time: time(entry.resets_at) })}`, warn: true };
  }
  const to = name(entry.provider, entry.to);
  const { why } = entry;
  const head =
    why.kind === 'manual'
      ? t('switch.notice.manual', { provider, to })
      : t('switch.notice.auto', {
          provider,
          from: name(entry.provider, entry.from),
          to,
          cause: t('switch.notice.causeRateLimit'),
        });
  const parts = [
    entry.moved > 0 ? t('switch.notice.moved', { n: entry.moved }) : null,
    entry.later > 0 ? t('switch.notice.later', { n: entry.later }) : null,
    entry.failed > 0 ? t('switch.notice.failed', { n: entry.failed }) : null,
  ].filter((p) => p !== null);
  return { text: [head, ...parts].join(' · '), warn: false };
}

export function rowStateKey(busy: boolean) {
  return busy ? ('switch.dialog.busy' as const) : ('switch.dialog.idle' as const);
}
