import { describe, expect, it } from 'vitest';
import type { HumanWait } from './gen/HumanWait';
import type { NodeView } from './gen/NodeView';
import type { WaitKind } from './gen/WaitKind';
import { asking, bySince, countsByTone, minimapClass, oldestWait, showsId, summaryOf, toneOf } from './notice';

const wait = (id: number, kind: WaitKind, since: number, text = ''): HumanWait => ({ id, kind, text, since, answerable: false, doc: null, images: [] });
const node = (waits: HumanWait[]) => ({ waits, state: { kind: 'running' } }) as unknown as NodeView;

describe('toneOf', () => {
  it('splits done from every kind that needs an answer', () => {
    expect(toneOf('done')).toBe('done');
    for (const kind of ['question', 'approval', 'trust', 'auth', 'input'] as const) expect(toneOf(kind)).toBe('ask');
  });
});

describe('showsId', () => {
  it('shows the Q number only for questions', () => {
    expect(showsId('question')).toBe(true);
    for (const kind of ['approval', 'trust', 'auth', 'input', 'done'] as const) expect(showsId(kind)).toBe(false);
  });
});

describe('order', () => {
  it('puts the oldest wait first even when it is a done notice', () => {
    const waits = [wait(2, 'question', 200), wait(1, 'done', 100), wait(3, 'done', 300)];
    expect([...waits].sort(bySince).map((w) => w.id)).toEqual([1, 2, 3]);
    expect(oldestWait(node(waits)).id).toBe(1);
  });
});

describe('counts', () => {
  it('counts each tone apart and leaves out empty tones', () => {
    const waits = [wait(1, 'question', 1), wait(2, 'approval', 2), wait(3, 'done', 3)];
    expect(countsByTone(waits)).toEqual([
      { tone: 'ask', n: 2 },
      { tone: 'done', n: 1 },
    ]);
    expect(countsByTone([wait(4, 'done', 4)])).toEqual([{ tone: 'done', n: 1 }]);
    expect(asking(node(waits)).map((w) => w.id)).toEqual([1, 2]);
  });
});

describe('minimapClass', () => {
  it('uses the tone of the oldest wait, else the run state', () => {
    expect(minimapClass(node([wait(1, 'done', 1), wait(2, 'question', 2)]))).toBe('done');
    expect(minimapClass(node([wait(2, 'approval', 2)]))).toBe('ask');
    expect(minimapClass(node([]))).toBe('running');
  });
});

describe('summaryOf', () => {
  it('drops link targets and is empty for a wait without text', () => {
    expect(summaryOf(wait(1, 'done', 1, 'see [plan](https://x.test/p)'))).toBe('see plan');
    expect(summaryOf(wait(2, 'approval', 1))).toBe('');
  });
});
