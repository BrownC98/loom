import { describe, expect, it } from 'vitest';
import type { NodeView } from './gen/NodeView';
import { arrangeTree, centerSide, crossed, edgeSides, freeSpot, placeGroups, push, TREE_SIDES, type ArrangeDirection, type Box, type GroupSlot } from './layout';

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

describe('arrangeTree (parent centered over the span of its children)', () => {
  // 1 -> {2, 3}, 2 -> {4}
  const tree = [view(1, null), view(2, 1), view(3, 1), view(4, 2)];

  it('down: children at the next depth, the parent centered over them', () => {
    const at = arrangeTree(tree, sized, 'down');
    // children of 1: 100 + 20 + 100 = 220 wide, so 1 (100 wide) sits at (220 - 100) / 2 = 60.
    expect(at.get(1)).toEqual({ x: 60, y: 0 });
    expect(at.get(2)).toEqual({ x: 0, y: 96 });
    expect(at.get(3)).toEqual({ x: 120, y: 96 });
    // 4 is the only child of 2: centered under it.
    expect(at.get(4)).toEqual({ x: 0, y: 192 });
  });

  it('right: depth along x, siblings along y, the parent centered', () => {
    const at = arrangeTree(tree, sized, 'right');
    expect(at.get(1)).toEqual({ x: 0, y: 35 });
    expect(at.get(2)).toEqual({ x: 146, y: 0 });
    expect(at.get(3)).toEqual({ x: 146, y: 70 });
    expect(at.get(4)).toEqual({ x: 292, y: 0 });
  });

  it('up mirrors down along depth; left mirrors right', () => {
    const down = arrangeTree(tree, sized, 'down');
    const up = arrangeTree(tree, sized, 'up');
    const right = arrangeTree(tree, sized, 'right');
    const left = arrangeTree(tree, sized, 'left');
    for (const id of [1, 2, 3, 4]) {
      expect(up.get(id)).toEqual({ x: down.get(id)!.x, y: 192 - down.get(id)!.y });
      expect(left.get(id)).toEqual({ x: 292 - right.get(id)!.x, y: right.get(id)!.y });
    }
  });

  it('roots of the group sit side by side at depth 0, in id order', () => {
    const at = arrangeTree([view(1, null), view(2, null), view(3, 1)], sized, 'down');
    expect(at.get(1)).toEqual({ x: 0, y: 0 });
    expect(at.get(3)).toEqual({ x: 0, y: 96 });
    expect(at.get(2)).toEqual({ x: 120, y: 0 });
  });

  it('a parent outside the group makes the node a root', () => {
    const at = arrangeTree([view(5, 99), view(6, null)], sized, 'down');
    expect(at.get(5)).toEqual({ x: 0, y: 0 });
    expect(at.get(6)).toEqual({ x: 120, y: 0 });
  });

  it('mixed sizes still do not overlap', () => {
    const big = { width: 200, height: 80 };
    const size = (id: number) => (id === 2 || id === 4 ? big : sized());
    const views = [view(1, null), view(2, 1), view(3, 1), view(4, 3), view(5, 2)];
    for (const direction of ['down', 'right', 'up', 'left'] as const) {
      const at = arrangeTree(views, size, direction);
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
  const g = (parent: number | null, extra: Partial<GroupSlot> = {}): GroupSlot => ({ size: { width: 100, height: 50 }, parent, ...extra });
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

});

describe('arrangeTree packing (plan A, docs/mockups/repo-drag.html)', () => {
  it("a leaf root sits beside a parent at depth 0, not beside the parent's children row", () => {
    // 사람의 loom 그래프 모양: 1의 자식 2, 3. 4는 자식 없는 뿌리. 폭 640, 카드 틈 20.
    const wide = () => ({ width: 640, height: 50 });
    const at = arrangeTree([view(1, null), view(2, 1), view(3, 1), view(4, null)], wide, 'down');
    // 자식 줄: 0..640, 660..1300 → 1은 가운데 330. 4는 첫 단계에서 1 바로 오른쪽: 330 + 640 + 20(지금은 1300 + 20).
    expect(at.get(1)!.x).toBe(330);
    expect(at.get(4)).toEqual({ x: 990, y: 0 });
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
