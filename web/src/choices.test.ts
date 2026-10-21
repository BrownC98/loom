import { describe, expect, it } from 'vitest';
import { choices } from './choices';

describe('choices', () => {
  it('reads the standard form and marks the recommended option', () => {
    expect(choices('Approve the mockup? (A: approve / B: change, Recommended: A)')).toEqual([
      { key: 'A', label: 'approve', recommended: true },
      { key: 'B', label: 'change', recommended: false },
    ]);
    // 「권장」 낱말은 사람의 언어라 아무 낱말이든 받는다.
    expect(choices('Merge? (A: now / B: later / C: never, Empfohlen: C)').map((c) => c.recommended)).toEqual([false, false, true]);
  });

  it('keeps options without a recommendation and commas inside labels', () => {
    expect(choices('Which? (A: red, blue / B: green)')).toEqual([
      { key: 'A', label: 'red, blue', recommended: false },
      { key: 'B', label: 'green', recommended: false },
    ]);
  });

  it('gives nothing for other questions', () => {
    expect(choices('Delete 3 worktrees?')).toEqual([]);
    expect(choices('Delete? (note: no undo)')).toEqual([]);
    expect(choices('(A: only one)')).toEqual([]);
    expect(choices('(A: x / B: y) and more text')).toEqual([]);
  });
});
