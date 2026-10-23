import { describe, expect, it } from 'vitest';
import { FIRST_LANES, lanes } from './commitLanes';
import type { Commit } from './gen/Commit';

const commit = (hash: string, ...parents: string[]): Commit => ({ hash, parents, subject: hash, author: 't', time: 0, refs: [] });

const history = [commit('m', 'b', 'c'), commit('b', 'd'), commit('c', 'd'), commit('d', 'e', 'f'), commit('e', 'a'), commit('f', 'a'), commit('a')];

describe('lanes', () => {
  it('lanes computed page by page equal lanes computed at once', () => {
    const whole = lanes(history, FIRST_LANES);
    for (let cut = 1; cut < history.length; cut++) {
      const head = lanes(history.slice(0, cut), FIRST_LANES);
      const tail = lanes(history.slice(cut), head.end);
      expect([...head.rows, ...tail.rows]).toEqual(whole.rows);
      expect(tail.end).toEqual(whole.end);
    }
  });

  it('lanes that cross a page boundary continue on the next page', () => {
    const head = lanes(history.slice(0, 2), FIRST_LANES);
    expect(head.end.open.map((l) => l.id)).toEqual(['d', 'c']);
    const tail = lanes(history.slice(2), head.end);
    expect(tail.rows[0].input.map((l) => l.id)).toEqual(['d', 'c']);
  });
});
