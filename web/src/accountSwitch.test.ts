import type { TFunction } from 'i18next';
import { describe, expect, it } from 'vitest';
import { isAutoSwitch, latestSwitch, noticeOf, rowStateKey, type SwitchEntry } from './accountSwitch';
import type { EventRecord } from './gen/EventRecord';

// 키와 값을 그대로 이어 붙이는 가짜 번역 함수. 문구 선택만 본다.
const t = ((key: string, o?: Record<string, unknown>) => (o ? `${key}${Object.values(o).map((v) => `|${v}`).join('')}` : key)) as unknown as TFunction;
const name = (_p: string, id: string | null) => (id === null ? 'SYS' : `acct${id}`);
const time = (s: number) => `T${s}`;

const switched = (over: Partial<Extract<SwitchEntry, { kind: 'accountSwitched' }>>): SwitchEntry => ({
  kind: 'accountSwitched',
  provider: 'claude',
  from: '1',
  to: '2',
  why: { kind: 'manual' },
  moved: 0,
  later: 0,
  failed: 0,
  ...over,
});

describe('noticeOf', () => {
  it('manual switch lists moved and later counts', () => {
    const n = noticeOf(switched({ moved: 2, later: 1 }), name, time, t);
    expect(n.text).toBe('switch.notice.manual|switch.provider.claude|acct2 · switch.notice.moved|2 · switch.notice.later|1');
    expect(n.warn).toBe(false);
  });

  it('auto usage switch names the old account and the window', () => {
    const n = noticeOf(switched({ why: { kind: 'usage', window: 'session', used: 96 }, moved: 3 }), name, time, t);
    expect(n.text).toContain('switch.notice.auto|switch.provider.claude|acct1|acct2|switch.notice.causeUsage|usage.window.session|96');
  });

  it('rate limit cause and null account use the system default name', () => {
    const n = noticeOf(switched({ why: { kind: 'rateLimit' }, from: null }), name, time, t);
    expect(n.text).toContain('SYS');
    expect(n.text).toContain('switch.notice.causeRateLimit');
  });

  it('failed nodes add a count and a warning', () => {
    const n = noticeOf(switched({ failed: 1 }), name, time, t);
    expect(n.text.endsWith('switch.notice.failed|1')).toBe(true);
    expect(n.warn).toBe(true);
  });

  it('no account left shows the earliest reset when known', () => {
    expect(noticeOf({ kind: 'noAccountLeft', provider: 'codex', resets_at: 100 }, name, time, t)).toEqual({
      text: 'switch.notice.noneLeft|switch.provider.codex · switch.notice.resetsAt|T100',
      warn: true,
    });
    expect(noticeOf({ kind: 'noAccountLeft', provider: 'codex', resets_at: null }, name, time, t).text).toBe('switch.notice.noneLeft|switch.provider.codex');
  });
});

describe('records', () => {
  const rec = (id: number, entry: EventRecord['entry']): EventRecord => ({ id, at: 0, entry, body: null });
  it('latestSwitch picks the first switch record of a newest-first log', () => {
    const log = [rec(5, { kind: 'removed', node: { id: 1, name: 'a' } }), rec(4, switched({})), rec(3, switched({ to: '9' }))];
    expect(latestSwitch(log)?.id).toBe(4);
    expect(latestSwitch([log[0]])).toBeNull();
  });

  it('only usage and rate-limit switches are automatic', () => {
    expect(isAutoSwitch(switched({}))).toBe(false);
    expect(isAutoSwitch(switched({ why: { kind: 'rateLimit' } }))).toBe(true);
    expect(isAutoSwitch({ kind: 'noAccountLeft', provider: 'claude', resets_at: null })).toBe(false);
  });

  it('dialog row text depends on busy', () => {
    expect(rowStateKey(true)).toBe('switch.dialog.busy');
    expect(rowStateKey(false)).toBe('switch.dialog.idle');
  });
});
