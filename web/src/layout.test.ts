import { describe, expect, it } from 'vitest';
import type { NodeView } from './gen/NodeView';
import { arrangeTree, edgeSides, growBox, placeGroups, reorder, TREE_SIDES, type ArrangeDirection, type Box, type GroupSlot } from './layout';

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
const created = () => 0;

describe('arrangeTree (parent centered over the span of its children)', () => {
  // 1 -> {2, 3}, 2 -> {4}
  const tree = [view(1, null), view(2, 1), view(3, 1), view(4, 2)];

  it('down: children at the next depth, the parent centered over them', () => {
    const at = arrangeTree(tree, sized, created, 'down');
    // children of 1: 100 + 20 + 100 = 220 wide, so 1 (100 wide) sits at (220 - 100) / 2 = 60.
    expect(at.get(1)).toEqual({ x: 60, y: 0 });
    expect(at.get(2)).toEqual({ x: 0, y: 96 });
    expect(at.get(3)).toEqual({ x: 120, y: 96 });
    // 4 is the only child of 2: centered under it.
    expect(at.get(4)).toEqual({ x: 0, y: 192 });
  });

  it('right: depth along x, siblings along y, the parent centered', () => {
    const at = arrangeTree(tree, sized, created, 'right');
    expect(at.get(1)).toEqual({ x: 0, y: 35 });
    expect(at.get(2)).toEqual({ x: 146, y: 0 });
    expect(at.get(3)).toEqual({ x: 146, y: 70 });
    expect(at.get(4)).toEqual({ x: 292, y: 0 });
  });

  it('up mirrors down along depth; left mirrors right', () => {
    const down = arrangeTree(tree, sized, created, 'down');
    const up = arrangeTree(tree, sized, created, 'up');
    const right = arrangeTree(tree, sized, created, 'right');
    const left = arrangeTree(tree, sized, created, 'left');
    for (const id of [1, 2, 3, 4]) {
      expect(up.get(id)).toEqual({ x: down.get(id)!.x, y: 192 - down.get(id)!.y });
      expect(left.get(id)).toEqual({ x: 292 - right.get(id)!.x, y: right.get(id)!.y });
    }
  });

  it('roots of the group sit side by side at depth 0, in id order', () => {
    const at = arrangeTree([view(1, null), view(2, null), view(3, 1)], sized, created, 'down');
    expect(at.get(1)).toEqual({ x: 0, y: 0 });
    expect(at.get(3)).toEqual({ x: 0, y: 96 });
    expect(at.get(2)).toEqual({ x: 120, y: 0 });
  });

  it('a parent outside the group makes the node a root', () => {
    const at = arrangeTree([view(5, 99), view(6, null)], sized, created, 'down');
    expect(at.get(5)).toEqual({ x: 0, y: 0 });
    expect(at.get(6)).toEqual({ x: 120, y: 0 });
  });

  it('mixed sizes still do not overlap', () => {
    const big = { width: 200, height: 80 };
    const size = (id: number) => (id === 2 || id === 4 ? big : sized());
    const views = [view(1, null), view(2, 1), view(3, 1), view(4, 3), view(5, 2)];
    for (const direction of ['down', 'right', 'up', 'left'] as const) {
      const at = arrangeTree(views, size, created, direction);
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

describe('placeGroups', () => {
  const g = (parent: number | null, extra: Partial<GroupSlot> = {}): GroupSlot => ({ size: { width: 100, height: 50 }, parent, rank: 0, ...extra });
  const start = { x: 10, y: 20 };

  it('roots sit side by side in order with GROUP_GAP 32', () => {
    expect(placeGroups([g(null), g(null)], 'down', start)).toEqual([
      { x: 10, y: 20 },
      { x: 142, y: 20 },
    ]);
  });

  it('child groups sit one level down and the parent is centered over their span', () => {
    const at = placeGroups([g(null), g(0), g(0)], 'down', start);
    // children span 100 + 32 + 100 = 232, parent (100 wide) is centered: offset 66.
    expect(at[0]).toEqual({ x: 76, y: 20 });
    expect(at[1]).toEqual({ x: 10, y: 102 });
    expect(at[2]).toEqual({ x: 142, y: 102 });
  });

  it('right puts levels along x and siblings along y', () => {
    const at = placeGroups([g(null), g(0), g(0)], 'right', start);
    expect(at[1]).toEqual({ x: 142, y: 20 });
    expect(at[2]).toEqual({ x: 142, y: 102 });
    // children span 50 + 32 + 50 = 132 along y, parent (50 tall) is centered: offset 41.
    expect(at[0]).toEqual({ x: 10, y: 61 });
  });

  it('up and left mirror the level order', () => {
    const up = placeGroups([g(null), g(0)], 'up', start);
    expect(up[0].y).toBeGreaterThan(up[1].y);
    const left = placeGroups([g(null), g(0)], 'left', start);
    expect(left[0].x).toBeGreaterThan(left[1].x);
  });

  it('a lower rank puts a group first among its siblings', () => {
    expect(placeGroups([g(null, { rank: 1 }), g(null, { rank: 0 })], 'down', start)).toEqual([
      { x: 142, y: 20 },
      { x: 10, y: 20 },
    ]);
  });
});

describe('arrangeTree rank', () => {
  it('siblings follow rank, then creation order; children follow their parent', () => {
    // 1 -> 3, 2 is a root. Rank puts 2 before 1.
    const rank = (id: number) => (id === 2 ? 0 : Infinity);
    const at = arrangeTree([view(1, null), view(2, null), view(3, 1), view(4, null)], sized, rank, 'down');
    expect(at.get(2)).toEqual({ x: 0, y: 0 });
    expect(at.get(1)).toEqual({ x: 120, y: 0 });
    expect(at.get(3)).toEqual({ x: 120, y: 96 });
    expect(at.get(4)).toEqual({ x: 240, y: 0 });
  });
});

// 사람 지시 "순서 바꾸기로 구현하는게 좋겠어". 형제 a(0..100), b(120..220), c(240..340). 중심은 50, 170, 290이다.
describe('reorder', () => {
  const sib = (key: string, x: number, y = 0) => ({ key, ...box(x, y) });
  const row = [sib('a', 0), sib('b', 120), sib('c', 240)];

  it('before the center line of the next sibling: no change', () => {
    // a 중심 50 + 119 = 169 < 170.
    expect(reorder(row, sib('a', 119), 'down')).toEqual(['a', 'b', 'c']);
  });

  it('past the center line: the order changes at once', () => {
    expect(reorder(row, sib('a', 121), 'down')).toEqual(['b', 'a', 'c']);
    expect(reorder(row, sib('a', 241), 'down')).toEqual(['b', 'c', 'a']);
    expect(reorder(row, sib('c', -1), 'down')).toEqual(['c', 'a', 'b']);
  });

  it('after a swap the passed sibling takes the old slot, so the order does not flip back at the same point', () => {
    // b와 a를 바꾼 뒤 배치: b(0..100), a(120..220). a는 여전히 121에 있다.
    expect(reorder([sib('b', 0), sib('a', 120), sib('c', 240)], sib('a', 121), 'down')).toEqual(['b', 'a', 'c']);
  });

  it('only the lane axis counts', () => {
    // 아래로 정렬이면 y는 보지 않는다.
    expect(reorder(row, sib('a', 50, 900), 'up')).toEqual(['a', 'b', 'c']);
    // 오른쪽으로 정렬이면 형제는 y로 늘어선다.
    const column = [sib('a', 0, 0), sib('b', 0, 70), sib('c', 0, 140)];
    expect(reorder(column, sib('a', 900, 71), 'right')).toEqual(['b', 'a', 'c']);
    expect(reorder(column, sib('a', 0, 69), 'left')).toEqual(['a', 'b', 'c']);
  });

  it('wider siblings use their own center line', () => {
    const mixed = [sib('a', 0), { key: 'b', ...box(120, 0, 400, 50) }];
    // b 중심 320. a 중심이 그 선을 넘어야 한다.
    expect(reorder(mixed, sib('a', 269), 'down')).toEqual(['a', 'b']);
    expect(reorder(mixed, sib('a', 271), 'down')).toEqual(['b', 'a']);
  });
});

// 끄는 카드를 담게 넓힌 묶음. 묶음 300×200, 틈은 위 60, 옆·아래 20.
describe('growBox', () => {
  const size = { width: 300, height: 200 };
  const inset = { top: 60, side: 20 };

  it('a card inside the box: no change', () => {
    expect(growBox(size, box(20, 60), inset)).toEqual({ lead: { x: 0, y: 0 }, size });
  });

  it('right and down: the box grows to the card plus the inset', () => {
    expect(growBox(size, box(250, 190), inset)).toEqual({ lead: { x: 0, y: 0 }, size: { width: 370, height: 260 } });
  });

  it('left and up: the box grows by lead, so the head stays above the card', () => {
    // 카드가 왼쪽 테두리 밖 -40, 머리 위 -30에 있다.
    expect(growBox(size, box(-40, -30), inset)).toEqual({ lead: { x: 60, y: 90 }, size: { width: 360, height: 290 } });
  });
});
