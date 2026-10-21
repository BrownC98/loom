// 계정 전환의 순수 함수: 상태 줄 알림 문구와 확인 창 줄 문구(목업 docs/mockups/account-switch.html ①③).
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

/** 최근 것부터 놓인 기록 중 가장 새 전환 기록. */
export function latestSwitch(log: EventRecord[]): (EventRecord & { entry: SwitchEntry }) | null {
  return (log.find((r) => isSwitchEntry(r.entry)) as (EventRecord & { entry: SwitchEntry }) | undefined) ?? null;
}

/** 사용량이나 한도 오류로 데몬이 스스로 바꾼 기록이다. 이때 화면이 설정과 사용량을 다시 읽는다. */
export function isAutoSwitch(entry: Entry): boolean {
  return entry.kind === 'accountSwitched' && entry.why.kind !== 'manual';
}

export type Notice = { text: string; warn: boolean };

/** 알림 한 줄. `name`은 계정 이름(null은 시스템 기본), `time`은 초기화 시각(유닉스 초)의 글자다. */
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
          cause: why.kind === 'usage' ? t('switch.notice.causeUsage', { window: t(`usage.window.${why.window}`), used: why.used }) : t('switch.notice.causeRateLimit'),
        });
  const parts = [
    entry.moved > 0 ? t('switch.notice.moved', { n: entry.moved }) : null,
    entry.later > 0 ? t('switch.notice.later', { n: entry.later }) : null,
    entry.failed > 0 ? t('switch.notice.failed', { n: entry.failed }) : null,
  ].filter((p) => p !== null);
  return { text: [head, ...parts].join(' · '), warn: entry.failed > 0 };
}

/** 확인 창 줄의 상태 글: 쉬는 노드는 바로, 일하는 노드는 턴이 끝나면 바꾼다. */
export function rowStateKey(busy: boolean) {
  return busy ? ('switch.dialog.busy' as const) : ('switch.dialog.idle' as const);
}
