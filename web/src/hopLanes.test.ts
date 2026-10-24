import { describe, expect, it } from 'vitest';
import { pickColor, siblingLanes, type HopPair } from './hopLanes';
import { ARRANGE_DIRECTIONS, type ArrangeDirection, type Box } from './layout';

const box = (x: number, y: number, width = 600, height = 437): Box => ({ at: { x, y }, size: { width, height } });
const PARENT = box(1064, 167);
const SIBLINGS = [box(56, 716), box(728, 716), box(1400, 716), box(2072, 716)];

function turn(b: Box, direction: ArrangeDirection): Box {
  const { x, y } = b.at;
  const { width: w, height: h } = b.size;
  switch (direction) {
    case 'down':
      return b;
    case 'up':
      return box(x, -(y + h), w, h);
    case 'right':
      return box(y, x, h, w);
    case 'left':
      return box(-(y + h), x, h, w);
  }
}

const pair = (cards: Box[], from: number, to: number): HopPair => ({ key: `${from}-${to}`, source: { id: from, box: cards[from] }, target: { id: to, box: cards[to] } });

function crossesInside(a: { x: number; y: number }, b: { x: number; y: number }, r: Box): boolean {
  const l = r.at.x + 0.5, t = r.at.y + 0.5, rr = r.at.x + r.size.width - 0.5, bb = r.at.y + r.size.height - 0.5;
  if (a.x === b.x) return a.x > l && a.x < rr && Math.max(a.y, b.y) > t && Math.min(a.y, b.y) < bb;
  return a.y > t && a.y < bb && Math.max(a.x, b.x) > l && Math.min(a.x, b.x) < rr;
}

describe('siblingLanes', () => {
  it.each(ARRANGE_DIRECTIONS)('keeps every sibling line out of the parent and all cards (%s)', (direction) => {
    const parent = turn(PARENT, direction);
    const cards = SIBLINGS.map((b) => turn(b, direction));
    const pairs = [pair(cards, 0, 1), pair(cards, 2, 0), pair(cards, 1, 3), pair(cards, 0, 3)];
    const routes = siblingLanes(parent, pairs, direction);
    expect(routes.size).toBe(pairs.length);
    for (const route of routes.values()) {
      for (let i = 1; i < route.points.length; i++) {
        for (const card of [parent, ...cards]) expect(crossesInside(route.points[i - 1], route.points[i], card)).toBe(false);
      }
    }
  });

  it('gives each pair its own lane, the shortest pair nearest the cards', () => {
    const pairs = [pair(SIBLINGS, 2, 0), pair(SIBLINGS, 0, 1), pair(SIBLINGS, 1, 3)];
    const routes = siblingLanes(PARENT, pairs, 'down');
    const lane = (key: string) => routes.get(key)!.points[1].y;
    expect(lane('0-1')).toBeGreaterThan(lane('2-0'));
    expect(lane('2-0')).toBeGreaterThan(lane('1-3'));
    expect(lane('1-3')).toBeGreaterThan(PARENT.at.y + PARENT.size.height);
  });

  it('starts two lines from one card at different points', () => {
    const routes = siblingLanes(PARENT, [pair(SIBLINGS, 0, 1), pair(SIBLINGS, 2, 0)], 'down');
    expect(routes.get('0-1')!.points[0].x).not.toBe(routes.get('2-0')!.points[3].x);
  });

  it('squeezes lanes into the gap when more pairs than lanes fit', () => {
    const cards = Array.from({ length: 12 }, (_, i) => box(i * 620, 716));
    const pairs = cards.slice(1).map((_, i) => pair(cards, 0, i + 1));
    const routes = siblingLanes(PARENT, pairs, 'down');
    const lanes = new Set([...routes.values()].map((r) => r.points[1].y));
    expect(lanes.size).toBe(pairs.length);
    for (const y of lanes) {
      expect(y).toBeGreaterThan(PARENT.at.y + PARENT.size.height);
      expect(y).toBeLessThan(716);
    }
  });

  it('leaves a pair that is not below the parent to the curve', () => {
    expect(siblingLanes(PARENT, [pair([box(0, 300), box(700, 300)], 0, 1)], 'down').size).toBe(0);
  });
});

describe('pickColor', () => {
  it('takes the least used color, the lowest on a tie', () => {
    expect(pickColor([])).toBe(0);
    expect(pickColor([0, 2])).toBe(1);
    expect(pickColor([0, 1, 2, 3, 4])).toBe(0);
    expect(pickColor([0, 1, 2, 3, 4, 0])).toBe(1);
  });
});
