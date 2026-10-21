import { describe, expect, it } from 'vitest';
import type { NodeView } from './gen/NodeView';
import { arrangeTree, centerSide, crossed, edgeSides, freeSpot, GROUP_GAP, grew, overlaps, placeGroups, push, sameSpots, stepAt, TREE_SIDES, type ArrangeDirection, type Box, type GroupSlot } from './layout';

const view = (id: number, parent: number | null): NodeView => ({
  id,
  name: `n${id}`,
  agent: null,
  parent,
  state: { kind: 'running' },
  unread: 0,
  held: 0,
  model: null,
  effort: null,
  place: { repo: 1, worktree: '/w' },
  topic: null,
});

const box = (x: number, y: number, width = 100, height = 50): Box => ({ at: { x, y }, size: { width, height } });

describe('edgeSides', () => {
  const parent = box(0, 0);
  const directions: ArrangeDirection[] = ['down', 'up', 'right', 'left'];
  const ahead: Record<ArrangeDirection, Box> = {
    down: box(0, 200),
    up: box(0, -200),
    right: box(300, 0),
    left: box(-300, 0),
  };

  it.each(directions)('child ahead of the parent uses TREE_SIDES[%s]', (direction) => {
    expect(edgeSides(parent, ahead[direction], direction)).toEqual(TREE_SIDES[direction]);
  });

  it('child touching the parent edge still counts as ahead', () => {
    expect(edgeSides(parent, box(0, 50), 'down')).toEqual(TREE_SIDES.down);
  });

  it('child not ahead: the axis with the larger empty gap wins', () => {
    // direction down, but the child sits far to the right with a small vertical gap.
    expect(edgeSides(parent, box(400, 20), 'down')).toEqual(TREE_SIDES.right);
    // far to the left.
    expect(edgeSides(parent, box(-400, 20), 'down')).toEqual(TREE_SIDES.left);
    // direction right, but the child sits far above.
    expect(edgeSides(parent, box(20, -400), 'right')).toEqual(TREE_SIDES.up);
    // far below.
    expect(edgeSides(parent, box(20, 400), 'right')).toEqual(TREE_SIDES.down);
  });

  it('horizontally overlapping cards use the vertical axis, not the center distance', () => {
    // centers are 300 apart horizontally and 70 vertically, but the cards overlap in x: the only empty gap is vertical.
    expect(edgeSides(box(0, 0, 400, 50), box(300, 70, 400, 50), 'right')).toEqual(TREE_SIDES.down);
  });
});

const sized = () => ({ width: 100, height: 50 });
const none = new Map<number, number>();

describe('arrangeTree (parent centered over the span of its children)', () => {
  // 1 -> {2, 3}, 2 -> {4}
  const tree = [view(1, null), view(2, 1), view(3, 1), view(4, 2)];

  it('down: children at the next depth, the parent centered over them', () => {
    const at = arrangeTree(tree, sized, 'down', 46, none).cards;
    // children of 1: 100 + 20 + 100 = 220 wide, so 1 (100 wide) sits at (220 - 100) / 2 = 60. Rows: 50 + 46.
    expect(at.get(1)).toEqual({ x: 60, y: 0 });
    expect(at.get(2)).toEqual({ x: 0, y: 96 });
    expect(at.get(3)).toEqual({ x: 120, y: 96 });
    // 4 is the only child of 2: centered under it.
    expect(at.get(4)).toEqual({ x: 0, y: 192 });
  });

  it('right: depth along x, siblings along y, the parent centered', () => {
    const at = arrangeTree(tree, sized, 'right', 46, none).cards;
    expect(at.get(1)).toEqual({ x: 0, y: 35 });
    expect(at.get(2)).toEqual({ x: 146, y: 0 });
    expect(at.get(3)).toEqual({ x: 146, y: 70 });
    expect(at.get(4)).toEqual({ x: 292, y: 0 });
  });

  it('up mirrors down along depth; left mirrors right', () => {
    const down = arrangeTree(tree, sized, 'down', 46, none).cards;
    const up = arrangeTree(tree, sized, 'up', 46, none).cards;
    const right = arrangeTree(tree, sized, 'right', 46, none).cards;
    const left = arrangeTree(tree, sized, 'left', 46, none).cards;
    for (const id of [1, 2, 3, 4]) {
      expect(up.get(id)).toEqual({ x: down.get(id)!.x, y: 192 - down.get(id)!.y });
      expect(left.get(id)).toEqual({ x: 292 - right.get(id)!.x, y: right.get(id)!.y });
    }
  });

  it('siblings follow the input order, not the id', () => {
    const at = arrangeTree([view(2, null), view(1, null), view(3, 1)], sized, 'down', 46, none).cards;
    expect(at.get(2)).toEqual({ x: 0, y: 0 });
    expect(at.get(1)).toEqual({ x: 120, y: 0 });
    expect(at.get(3)).toEqual({ x: 120, y: 96 });
  });

  it('a parent outside the group makes the node a root', () => {
    const at = arrangeTree([view(5, 99), view(6, null)], sized, 'down', 46, none).cards;
    expect(at.get(5)).toEqual({ x: 0, y: 0 });
    expect(at.get(6)).toEqual({ x: 120, y: 0 });
  });

  it('a row is as tall as its tallest card; the gap between rows is constant', () => {
    // 1 (300 tall) and 2 (50) are roots; 3 is the child of 2.
    const size = (id: number) => (id === 1 ? { width: 100, height: 300 } : sized());
    const at = arrangeTree([view(1, null), view(2, null), view(3, 2)], size, 'down', 46, none).cards;
    expect(at.get(3)!.y).toBe(300 + 46);
  });

  it('mixed sizes still do not overlap', () => {
    const big = { width: 200, height: 80 };
    const size = (id: number) => (id === 2 || id === 4 ? big : sized());
    const views = [view(1, null), view(2, 1), view(3, 1), view(4, 3), view(5, 2)];
    for (const direction of ['down', 'right', 'up', 'left'] as const) {
      const at = arrangeTree(views, size, direction, 46, none).cards;
      const rects = views.map((v) => ({ ...at.get(v.id)!, ...size(v.id) }));
      for (const [i, a] of rects.entries()) {
        for (const b of rects.slice(i + 1)) {
          const apart = a.x + a.width <= b.x || b.x + b.width <= a.x || a.y + a.height <= b.y || b.y + b.height <= a.y;
          expect(apart).toBe(true);
        }
      }
    }
  });
});

// 목업 docs/mockups/arrange-wide.html 규칙 1·3·4.
describe('arrangeTree with two parents in one group', () => {
  const center = (at: Map<number, { x: number }>, id: number) => at.get(id)!.x + 50;
  const span = (at: Map<number, { x: number }>, ids: number[]) => ({ lo: Math.min(...ids.map((id) => at.get(id)!.x)), hi: Math.max(...ids.map((id) => at.get(id)!.x + 100)) });

  it.each([
    ['1 first', [1, 2]],
    ['2 first', [2, 1]],
  ])('each parent is centered over its children and the edges do not cross (%s)', (_, roots) => {
    const kids: Record<number, number[]> = { 1: [3, 4], 2: [5, 6, 7] };
    const views = [...roots.map((r) => view(r, null)), ...roots.flatMap((r) => kids[r].map((k) => view(k, r)))];
    const at = arrangeTree(views, sized, 'down', 46, none).cards;
    for (const r of roots) {
      const s = span(at, kids[r]);
      expect(center(at, r)).toBe((s.lo + s.hi) / 2);
    }
    // 부모 순서와 자식 줄 순서가 같다: 왼쪽 부모의 자식은 모두 오른쪽 부모의 자식보다 왼쪽이다.
    const [left, right] = roots;
    expect(center(at, left)).toBeLessThan(center(at, right));
    expect(span(at, kids[left]).hi).toBeLessThan(span(at, kids[right]).lo);
  });

  it('a parent with child groups stands over its band, and no card stands between them', () => {
    // 1은 다른 묶음에 자식이 있다(줄 폭 400). 2의 자식 3, 4, 5는 이 묶음 안이다.
    const views = [view(2, null), view(1, null), view(3, 2), view(4, 2), view(5, 2)];
    const laid = arrangeTree(views, sized, 'down', 46, new Map([[1, 400]]));
    const at = laid.cards;
    const band = { lo: laid.bands.get(1)!, hi: laid.bands.get(1)! + 400 };
    // 1은 자식 줄 자리의 가운데 위다.
    expect(center(at, 1)).toBe(band.lo + 200);
    expect(span(at, [3, 4, 5]).hi).toBeLessThanOrEqual(band.lo);
    expect(center(at, 2)).toBeLessThan(center(at, 1));
  });
});

describe('placeGroups', () => {
  const g = (parent: number | null, card: number | null = parent === null ? null : 7): GroupSlot => ({ parent, card });
  // `bands`는 줄 자리의 시작이다. 줄은 그 뒤 GROUP_GAP / 2에서 시작한다.
  const lay = (sizes: { width: number; height: number }[], bands: Record<number, number> = {}) => (i: number) => ({ size: sizes[i], band: (card: number) => bands[card] });
  const s = (width: number, height: number) => ({ width, height });

  it('roots sit side by side in order with GROUP_GAP 32', () => {
    expect(placeGroups([g(null), g(null)], lay([s(100, 50), s(100, 50)]), 'down')).toEqual([
      { x: 0, y: 0 },
      { x: 132, y: 0 },
    ]);
  });

  it('child groups sit in the band slot under the parent group', () => {
    const at = placeGroups([g(null), g(0), g(0)], lay([s(300, 100), s(100, 50), s(100, 50)], { 7: 93 }), 'down');
    // 줄 자리는 93에서 시작하고 줄은 93 + 16 = 109에서 시작한다.
    expect(at).toEqual([
      { x: 0, y: 0 },
      { x: 109, y: 132 },
      { x: 241, y: 132 },
    ]);
  });

  it('a band wider than the parent group pushes the parent group right', () => {
    const at = placeGroups([g(null), g(0), g(0), g(0), g(0)], lay([s(100, 50), ...Array(4).fill(s(100, 50))], { 7: -214 }), 'down');
    expect(at.map((p) => p.x)).toEqual([198, 0, 132, 264, 396]);
  });

  it('bands of two parent cards in one group keep GROUP_GAP (with arrangeTree)', () => {
    // 묶음 0의 뿌리 카드 7, 8이 각각 다른 묶음에 자식이 있다.
    const groups = [g(null), g(0, 7), g(0, 8)];
    const sizes = [s(0, 0), s(200, 50), s(200, 50)];
    const at = placeGroups(
      groups,
      (i, bands) => {
        if (i > 0) return { size: sizes[i], band: () => 0 };
        const laid = arrangeTree([view(7, null), view(8, null)], sized, 'down', 46, bands);
        return { size: s(laid.cards.get(8)!.x + 100, 50), band: (card) => laid.bands.get(card)! };
      },
      'down',
    );
    expect(at[1].x + 200 + 32).toBeLessThanOrEqual(at[2].x);
  });

  it('the gap under a parent group is GROUP_GAP even when the child is much taller (two repos of different heights)', () => {
    // 사람 화면: loom은 카드 높이가 비슷하고, ktasearch는 자식 묶음 카드가 훨씬 크다.
    for (const sizes of [[s(300, 400), s(300, 420)], [s(300, 200), s(300, 900)]]) {
      const at = placeGroups([g(null), g(0)], lay(sizes, { 7: 150 }), 'down');
      expect(at[1].y - (at[0].y + sizes[0].height)).toBe(32);
    }
  });

  it('right puts depth along x and the band along y', () => {
    const at = placeGroups([g(null), g(0), g(0)], lay([s(100, 300), s(100, 50), s(100, 50)], { 7: 68 }), 'right');
    expect(at).toEqual([
      { x: 0, y: 0 },
      { x: 132, y: 84 },
      { x: 132, y: 166 },
    ]);
  });

  it('up and left mirror the depth order', () => {
    const sizes = lay([s(100, 50), s(100, 50)], { 7: 50 });
    const up = placeGroups([g(null), g(0)], sizes, 'up');
    expect(up[0].y).toBeGreaterThan(up[1].y);
    const left = placeGroups([g(null), g(0)], sizes, 'left');
    expect(left[0].x).toBeGreaterThan(left[1].x);
  });
});

describe('arrangeTree packing (plan A, docs/mockups/repo-drag.html)', () => {
  it("a leaf root sits beside a parent at depth 0, not beside the parent's children row", () => {
    // 사람의 loom 그래프 모양: 1의 자식 2, 3. 4는 자식 없는 뿌리. 폭 640, 카드 틈 20.
    const wide = () => ({ width: 640, height: 50 });
    const at = arrangeTree([view(1, null), view(2, 1), view(3, 1), view(4, null)], wide, 'down', 46, none).cards;
    expect(at.get(1)!.x).toBe(330);
    expect(at.get(4)).toEqual({ x: 990, y: 0 });
  });
});

describe('arrangeTree packing (rule 5, docs/mockups/arrange-mixed.html v5)', () => {
  const card = () => ({ width: 600, height: 466 });

  it('a later tree does not go under an earlier tree deeper than its deepest card (screenshot shape ②)', () => {
    // main 묶음: 1은 자식 없는 뿌리, 2의 자식 3은 같은 묶음, 2의 자식 묶음 줄 폭 672(묶음 640 + 묶음 틈).
    const laid = arrangeTree([view(1, null), view(2, null), view(3, 2)], card, 'down', 112, new Map([[2, 672]]));
    const at = laid.cards;
    expect(at.get(3)).toEqual({ x: 620, y: 578 });
    expect(laid.bands.get(2)).toBe(1240);
    // 2는 자식 3과 자식 묶음 줄 전체의 가운데 위다.
    expect(at.get(2)!.x + 300).toBe((620 + 1240 + 672) / 2);
  });
});

/**
 * 그래프 자동 배치(Graph.tsx layGroup + placeGroups)를 같은 순서로 돌린다. 카드 600×466, 묶음 머리 60, 옆 20, 묶음 안 줄 틈 112.
 * 돌려주는 것은 카드의 그래프 기준 상자다. 묶음의 부모는 그 묶음 노드의 부모가 있는 다른 묶음이다.
 */
function arrangeGraph(nodes: { id: number; parent: number | null; group: number; size?: { width: number; height: number } }[], direction: ArrangeDirection) {
  const [side, top] = [20, 60];
  const vertical = direction === 'down' || direction === 'up';
  const size = (id: number) => nodes.find((n) => n.id === id)!.size ?? { width: 600, height: 466 };
  const groupIds = [...new Set(nodes.map((n) => n.group))];
  const groupOf = new Map(nodes.map((n) => [n.id, n.group]));
  const slots = groupIds.map((g): GroupSlot => {
    const hit = nodes.find((n) => n.group === g && n.parent !== null && groupOf.get(n.parent) !== g);
    return hit ? { parent: groupIds.indexOf(groupOf.get(hit.parent!)!), card: hit.parent } : { parent: null, card: null };
  });
  const local: Map<number, { x: number; y: number }>[] = [];
  const at = placeGroups(
    slots,
    (i, bands) => {
      const members = nodes.filter((n) => n.group === groupIds[i]);
      const ids = new Set(members.map((m) => m.id));
      const laid = arrangeTree(members.map((m) => view(m.id, m.parent !== null && ids.has(m.parent) ? m.parent : null)), size, direction, vertical ? top + side + GROUP_GAP : 2 * side + GROUP_GAP, bands);
      const ps = [...laid.cards];
      const mx = Math.min(...ps.map(([, p]) => p.x));
      const my = Math.min(...ps.map(([, p]) => p.y));
      local[i] = new Map(ps.map(([id, p]) => [id, { x: side + p.x - mx, y: top + p.y - my }]));
      const right = Math.max(...ps.map(([id, p]) => p.x + size(id).width));
      const bottom = Math.max(...ps.map(([id, p]) => p.y + size(id).height));
      return {
        size: { width: right - mx + 2 * side, height: bottom - my + top + side },
        band: (c) => (vertical ? side + laid.bands.get(c)! - mx : top + laid.bands.get(c)! - my),
      };
    },
    direction,
  );
  return new Map(
    nodes.map((n) => {
      const i = groupIds.indexOf(n.group);
      const p = local[i].get(n.id)!;
      return [n.id, { at: { x: at[i].x + p.x, y: at[i].y + p.y }, size: size(n.id) }];
    }),
  );
}

/** 나무 간선(계단)의 꼭짓점. 부모 변 가운데 → 꺾는 자리 → 자식 변 가운데. */
function stepPath(parent: Box, child: Box, direction: ArrangeDirection, bend: number) {
  const step = stepAt(parent, child, direction, bend);
  if (step === null) return null;
  const c = (b: Box) => ({ x: b.at.x + b.size.width / 2, y: b.at.y + b.size.height / 2 });
  const [p, q] = [c(parent), c(child)];
  if (direction === 'down' || direction === 'up') {
    const sign = direction === 'down' ? 1 : -1;
    const a = p.y + (sign * parent.size.height) / 2;
    const b = q.y - (sign * child.size.height) / 2;
    const at = a + (b - a) * step;
    return [{ x: p.x, y: a }, { x: p.x, y: at }, { x: q.x, y: at }, { x: q.x, y: b }];
  }
  const sign = direction === 'right' ? 1 : -1;
  const a = p.x + (sign * parent.size.width) / 2;
  const b = q.x - (sign * child.size.width) / 2;
  const at = a + (b - a) * step;
  return [{ x: a, y: p.y }, { x: at, y: p.y }, { x: at, y: q.y }, { x: b, y: q.y }];
}

describe('catalogue (docs/mockups/arrange-mixed.html v5): every tree edge is a step edge and crosses no card', () => {
  type N = { id: number; parent: number | null; group: number; size?: { width: number; height: number } };
  const n = (id: number, parent: number | null, group: number, size?: { width: number; height: number }): N => ({ id, parent, group, size });
  const screenshot = [n(1, null, 0), n(2, null, 0), n(3, 2, 0), n(4, 2, 1), n(5, 4, 1)];
  const cases: [string, N[], ArrangeDirection][] = [
    ['② mixed (screenshot)', screenshot, 'down'],
    ['③ in-group children only', [n(1, null, 0), n(2, 1, 0), n(3, 1, 0), n(4, 1, 0)], 'down'],
    ['④ one child group', [n(1, null, 0), n(2, 1, 1)], 'down'],
    ['⑤ many child groups', [n(1, null, 0), n(2, 1, 1), n(3, 1, 2), n(4, 1, 3)], 'down'],
    ['⑥ parent in the middle', [n(1, null, 0), n(2, null, 0), n(3, null, 0), n(4, 2, 0), n(5, 2, 1)], 'down'],
    ['⑦ deep chain', [n(1, null, 0), n(2, 1, 1), n(3, 2, 1), n(4, 2, 2), n(5, 2, 3), n(6, 5, 3)], 'down'],
    ['⑧ several root groups', [n(1, null, 0), n(2, null, 1), n(3, 2, 1), n(4, 1, 2)], 'down'],
    ['⑨ wide and tall cards', [n(1, null, 0), n(2, 1, 0, { width: 1000, height: 300 }), n(3, 1, 0, { width: 400, height: 800 }), n(4, 1, 1)], 'down'],
    ['⑩ M8', [n(1, null, 0), n(2, 1, 0), n(3, 1, 0), n(4, 1, 0), n(5, null, 0), n(6, 5, 1), n(7, 5, 2), n(8, 5, 3), n(9, 5, 4)], 'down'],
    ['⑪ mixed, right', screenshot, 'right'],
    ['⑪ mixed, up', screenshot, 'up'],
    ['⑪ mixed, left', screenshot, 'left'],
  ];
  const inside = (pt: { x: number; y: number }, b: Box) => pt.x > b.at.x && pt.x < b.at.x + b.size.width && pt.y > b.at.y && pt.y < b.at.y + b.size.height;

  it.each(cases)('%s', (_, nodes, direction) => {
    const boxes = arrangeGraph(nodes, direction);
    for (const [, a] of boxes) for (const [, b] of boxes) if (a !== b) expect(overlaps(a, b)).toBe(false);
    for (const child of nodes) {
      if (child.parent === null) continue;
      const path = stepPath(boxes.get(child.parent)!, boxes.get(child.id)!, direction, 36);
      expect(path).not.toBeNull();
      for (let s = 1; s < path!.length; s++) {
        const [p, q] = [path![s - 1], path![s]];
        for (let t = 0; t <= 200; t++) {
          const pt = { x: p.x + ((q.x - p.x) * t) / 200, y: p.y + ((q.y - p.y) * t) / 200 };
          for (const [id, b] of boxes) if (id !== child.id && id !== child.parent) expect(inside(pt, b), `edge ${child.parent}→${child.id} crosses ${id}`).toBe(false);
        }
      }
    }
  });

  it('the screenshot shape matches the approved picture ②', () => {
    const boxes = arrangeGraph(screenshot, 'down');
    expect([1, 2, 3, 4, 5].map((id) => boxes.get(id)!.at)).toEqual([
      { x: 20, y: 60 },
      { x: 986, y: 60 },
      { x: 640, y: 638 },
      { x: 1296, y: 1216 },
      { x: 1296, y: 1794 },
    ]);
  });
});

describe('stepAt', () => {
  it('bends `bend` past the parent, as a share of the gap', () => {
    expect(stepAt(box(0, 0), box(0, 162), 'down', 36)).toBe(36 / 112);
    expect(stepAt(box(0, 162), box(0, 0), 'up', 36)).toBe(36 / 112);
  });
  it('a gap narrower than two bends bends in the middle; a child behind gets a curve (null)', () => {
    expect(stepAt(box(0, 0), box(0, 100), 'down', 36)).toBe(0.5);
    expect(stepAt(box(0, 0), box(300, 0), 'down', 36)).toBeNull();
  });
});

describe('push (people decision 2026-10-21, mockup v4 ②)', () => {
  const sib = (key: string, x: number, y: number, width = 100, height = 50) => ({ key, ...box(x, y, width, height) });

  it('the overlapped sibling moves the shortest way out, with the gap', () => {
    // mover 0..100 × 0..50. a는 80..180 × 0..50: 오른쪽으로 20 + 32, 왼쪽 180 - (-32) = 212.
    expect(push([sib('a', 80, 0)], box(0, 0), ['a'], 32)).toEqual(new Map([['a', { x: 132, y: 0 }]]));
    // a가 아래로 조금만 겹치면 아래로 비킨다: 50 + 32 - 40 = 42.
    expect(push([sib('a', 0, 40, 400)], box(0, 0), ['a'], 32)).toEqual(new Map([['a', { x: 0, y: 82 }]]));
  });

  it('ties pick right, down, left, up in that order', () => {
    // 같은 자리, 같은 크기: 네 쪽 모두 132. 오른쪽이다.
    expect(push([sib('a', 0, 0, 100, 100)], box(0, 0, 100, 100), ['a'], 32)).toEqual(new Map([['a', { x: 132, y: 0 }]]));
  });

  it('a pushed sibling pushes the next one the same way (cascade)', () => {
    const got = push([sib('a', 80, 0), sib('b', 200, 0, 100, 300)], box(0, 0), ['a'], 32);
    // a → 132..232. b(200..)와 겹친다 → 같은 쪽(오른쪽)으로 232 + 32 = 264.
    expect(got).toEqual(new Map([['a', { x: 132, y: 0 }], ['b', { x: 264, y: 0 }]]));
  });

  it('only targets move first; other overlaps stay until a cascade reaches them', () => {
    expect(push([sib('a', 80, 0), sib('c', -50, 0)], box(0, 0), ['a'], 32)).toEqual(new Map([['a', { x: 132, y: 0 }]]));
  });
});

describe('crossed', () => {
  it('changes when the dragged center passes a center line', () => {
    const other = box(100, 0);
    const before = centerSide(box(0, 0), other);
    expect(crossed(before, centerSide(box(90, 0), other))).toBe(false);
    expect(crossed(before, centerSide(box(110, 0), other))).toBe(true);
    expect(crossed(before, centerSide(box(0, 10), other))).toBe(true);
  });
});

describe('freeSpot', () => {
  it('moves right past overlapped boxes with the gap', () => {
    expect(freeSpot(box(0, 0), [box(50, 0), box(182, 0)], 32)).toEqual({ x: 314, y: 0 });
    expect(freeSpot(box(0, 100), [box(50, 0)], 32)).toEqual({ x: 0, y: 100 });
  });
});

describe('growth without a drag (repro: new card in row 2 of main)', () => {
  it('grew lists only boxes that were there before and got wider or taller', () => {
    const size = (width: number, height: number) => ({ width, height });
    const before = new Map([['main', size(1000, 400)], ['side', size(500, 400)], ['small', size(300, 300)]]);
    const now = new Map([['main', size(1000, 840)], ['side', size(500, 400)], ['small', size(200, 400)], ['new', size(500, 400)]]);
    expect(grew(before, now)).toEqual(['main', 'small']);
  });

  it('a group that grows down pushes the saved group under it the shortest way, with the gap', () => {
    // main 0..1000 × 0..400 grows to 840 high (a card in row 2). 판정근거-범위 at 480..980 × 440..840.
    // right 1000 + 32 - 480 = 552, down 840 + 32 - 440 = 432, left 1012, up 872: down.
    const moved = push([{ key: 'g', ...box(480, 440, 500, 400) }], box(0, 0, 1000, 840), ['g'], 32);
    expect(moved).toEqual(new Map([['g', { x: 480, y: 872 }]]));
  });

  it('a new group whose auto spot is on a grown saved group goes to its right', () => {
    expect(freeSpot(box(480, 440, 500, 400), [box(0, 0, 1000, 840)], 32)).toEqual({ x: 1032, y: 440 });
  });
});

describe('the left tree grows children (target picture: two root trees, child groups under the right tree)', () => {
  type N = { id: number; parent: number | null; group: number };
  const n = (id: number, parent: number | null, group: number): N => ({ id, parent, group });
  // main 묶음(0): 1은 왼쪽 뿌리, 2 → 3. 3의 자식 묶음 1, 2.
  const target = [n(1, null, 0), n(2, null, 0), n(3, 2, 0), n(4, 3, 1), n(5, 3, 2)];
  const center = (b: Box) => b.at.x + b.size.width / 2;
  const span = (boxes: Map<number, Box>, ids: number[]) => ({ lo: Math.min(...ids.map((id) => boxes.get(id)!.at.x)), hi: Math.max(...ids.map((id) => boxes.get(id)!.at.x + boxes.get(id)!.size.width)) });

  it('the target picture: 1 and 2 side by side, 3 straight under 2, child groups centred under 3', () => {
    const at = arrangeGraph(target, 'down');
    expect(at.get(1)!.at.y).toBe(at.get(2)!.at.y);
    expect(center(at.get(3)!)).toBe(center(at.get(2)!));
    const kids = span(at, [4, 5]);
    expect((kids.lo + kids.hi) / 2).toBe(center(at.get(3)!));
    expect(at.get(4)!.at.y).toBe(at.get(5)!.at.y);
    expect(at.get(4)!.at.y).toBeGreaterThan(at.get(3)!.at.y + 466);
  });

  it.each([
    ['(a) one child in the same group', [n(6, 1, 0)], [6]],
    ['(b) one child group', [n(6, 1, 3)], [6]],
    ['(c) four children, wider than the gap', [n(6, 1, 0), n(7, 1, 0), n(8, 1, 0), n(9, 1, 0)], [6, 7, 8, 9]],
    ['(b+c) three child groups', [n(6, 1, 3), n(7, 1, 4), n(8, 1, 5)], [6, 7, 8]],
  ] as [string, N[], number[]][])('%s: 1 stays centred over its children, the right tree moves right as a whole', (_, added, kids) => {
    const before = arrangeGraph(target, 'down');
    const at = arrangeGraph([...target, ...added], 'down');
    for (const [a, x] of at) for (const [b, y] of at) if (a !== b) expect(overlaps(x, y), `${a} and ${b}`).toBe(false);
    const left = span(at, [1, ...kids]);
    expect(center(at.get(1)!)).toBe((span(at, kids).lo + span(at, kids).hi) / 2);
    const right = span(at, [2, 3, 4, 5]);
    expect(right.lo).toBeGreaterThan(left.hi);
    // 오른쪽 나무는 모양을 지키고 통째로 옮겨 간다.
    const shift = at.get(2)!.at.x - before.get(2)!.at.x;
    expect(shift).toBeGreaterThanOrEqual(0);
    for (const id of [3, 4, 5]) expect(at.get(id)!.at.x - before.get(id)!.at.x).toBe(shift);
    if (kids.length > 1) expect(shift).toBeGreaterThan(0);
  });
});

describe('sameSpots (an arranged repo saves the new auto layout when it differs)', () => {
  const m = (...e: [string, number][]) => new Map(e.map(([k, x]) => [k, { x, y: 0 }]));

  it('equal, or off by less than half a pixel: same', () => {
    expect(sameSpots(m(['1', 0], ['2', 700]), m(['1', 0], ['2', 700.3]))).toBe(true);
  });

  it('a moved box or a new box without a saved spot: not same', () => {
    expect(sameSpots(m(['1', 0], ['2', 700]), m(['1', 310], ['2', 1340]))).toBe(false);
    expect(sameSpots(m(['1', 0], ['2', 700]), m(['1', 0], ['2', 700], ['6', 0]))).toBe(false);
  });
});
