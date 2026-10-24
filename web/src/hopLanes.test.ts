import { describe, expect, it } from 'vitest';
import { pickColor, siblingLanes, type HopPair, type HopRoute } from './hopLanes';
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

const LABEL = { full: { width: 150, height: 21 }, short: { width: 24, height: 21 } };
const pair = (cards: Box[], from: number, to: number): HopPair => ({ key: `${from}-${to}`, source: { id: from, box: cards[from], room: cards[from] }, target: { id: to, box: cards[to], room: cards[to] }, label: LABEL });

type Rect = { l: number; t: number; r: number; b: number };
const overlap = (a: Rect, b: Rect) => a.l < b.r && b.l < a.r && a.t < b.b && b.t < a.b;
const rectOf = (b: Box): Rect => ({ l: b.at.x, t: b.at.y, r: b.at.x + b.size.width, b: b.at.y + b.size.height });

function placedLabels(routes: Map<string, HopRoute>, pairs: HopPair[], direction: ArrangeDirection): Rect[] {
  const vertical = direction === 'down' || direction === 'up';
  return pairs.flatMap((p) => {
    const label = routes.get(p.key)?.label;
    if (!label) return [];
    const size = label.short ? p.label.short : p.label.full;
    const w = vertical ? size.width : size.height;
    const h = vertical ? size.height : size.width;
    return [{ l: label.at.x - w / 2, t: label.at.y - h / 2, r: label.at.x + w / 2, b: label.at.y + h / 2 }];
  });
}

function expectLabelsClear(labels: Rect[], cards: Box[]) {
  labels.forEach((a, i) => labels.slice(i + 1).forEach((b) => expect(overlap(a, b)).toBe(false)));
  for (const label of labels) for (const card of cards) expect(overlap(label, rectOf(card))).toBe(false);
}

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

  it.each(ARRANGE_DIRECTIONS)('keeps full labels of nested and crossing pairs apart (%s)', (direction) => {
    const parent = turn(PARENT, direction);
    const cards = SIBLINGS.map((b) => turn(b, direction));
    const pairs = [pair(cards, 0, 3), pair(cards, 1, 2), pair(cards, 0, 2), pair(cards, 1, 3)];
    const routes = siblingLanes(parent, pairs, direction);
    for (const route of routes.values()) expect(route.label?.short).toBe(false);
    expectLabelsClear(placedLabels(routes, pairs, direction), [parent, ...cards]);
    pairs.forEach((p) => {
      const label = placedLabels(routes, [p], direction)[0];
      for (const other of pairs.filter((o) => o !== p)) {
        const [a, b, c, d] = routes.get(other.key)!.points;
        for (const [from, to] of [[a, b], [c, d]]) {
          const stem = { l: Math.min(from.x, to.x), t: Math.min(from.y, to.y), r: Math.max(from.x, to.x), b: Math.max(from.y, to.y) };
          expect(overlap(label, { l: stem.l - 1, t: stem.t - 1, r: stem.r + 1, b: stem.b + 1 })).toBe(false);
        }
      }
    });
  });

  it('keeps full two-way labels of adjacent pairs on simple cards apart', () => {
    const parent = box(250, 0, 600, 466);
    const cards = [0, 280, 560, 840].map((x) => box(x, 578, 260, 134));
    const label = { full: { width: 479, height: 21 }, short: { width: 40, height: 21 } };
    const pairs = [0, 1, 2].map((i): HopPair => ({ ...pair(cards, i, i + 1), label }));
    const routes = siblingLanes(parent, pairs, 'down');
    for (const route of routes.values()) expect(route.label?.short).toBe(false);
    expectLabelsClear(placedLabels(routes, pairs, 'down'), [parent, ...cards]);
  });

  it.each(ARRANGE_DIRECTIONS)('shortens or drops a label rather than cover another when the gap is full (%s)', (direction) => {
    const parent = turn(PARENT, direction);
    const cards = Array.from({ length: 8 }, (_, i) => turn(box(i * 280, 716, 260, 134), direction));
    const label = { full: { width: 479, height: 21 }, short: { width: 40, height: 21 } };
    const pairs = cards.slice(1).flatMap((_, i): HopPair[] => [{ ...pair(cards, i, i + 1), label }, ...(i + 2 < cards.length ? [{ ...pair(cards, i, i + 2), label }] : [])]);
    const routes = siblingLanes(parent, pairs, direction);
    expect(routes.size).toBe(pairs.length);
    expectLabelsClear(placedLabels(routes, pairs, direction), [parent, ...cards]);
  });

  it('keeps lanes above the worktree frame of a sibling', () => {
    const frame = (b: Box): Box => ({ at: { x: b.at.x - 20, y: b.at.y - 60 }, size: { width: b.size.width + 40, height: b.size.height + 80 } });
    const framed = (from: number, to: number): HopPair => ({ ...pair(SIBLINGS, from, to), source: { id: from, box: SIBLINGS[from], room: frame(SIBLINGS[from]) }, target: { id: to, box: SIBLINGS[to], room: frame(SIBLINGS[to]) } });
    const routes = siblingLanes(PARENT, [framed(0, 1), framed(2, 0), framed(1, 3), framed(0, 3), framed(1, 2), framed(2, 3)], 'down');
    expect(routes.size).toBe(6);
    for (const route of routes.values()) {
      expect(route.points[1].y).toBeLessThan(716 - 60);
      expect(route.points[0].y).toBe(716);
    }
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
