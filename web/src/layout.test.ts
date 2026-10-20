import { describe, expect, it } from 'vitest';
import type { NodeView } from './gen/NodeView';
import { arrangeTree, edgeSides, placeGroups, TREE_SIDES, type ArrangeDirection, type Box, type GroupSlot } from './layout';

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

const sized = () => ({ size: { width: 100, height: 50 } });

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
    const size = (id: number) => (id === 2 || id === 4 ? big : sized().size);
    const views = [view(1, null), view(2, 1), view(3, 1), view(4, 3), view(5, 2)];
    for (const direction of ['down', 'right', 'up', 'left'] as const) {
      const at = arrangeTree(views, (id) => ({ size: size(id) }), direction);
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

  it('a dragged group keeps its position and does not move the others', () => {
    const plain = placeGroups([g(null), g(0)], 'down', start);
    const dragged = placeGroups([g(null), g(0, { dragged: { x: 500, y: 600 } })], 'down', start);
    expect(dragged[1]).toEqual({ x: 500, y: 600 });
    expect(dragged[0]).toEqual(plain[0]);
  });
});

// 목업 docs/mockups/overlap.html 「원인」의 다섯 장면. 끈 것과 배치가 겹치면 형제가 늘어서는 쪽으로 민다.
describe('overlap after drag (untangle)', () => {
  const card = { width: 100, height: 50 };
  const g = (parent: number | null, extra: Partial<GroupSlot> = {}): GroupSlot => ({ size: { width: 100, height: 50 }, parent, ...extra });
  const start = { x: 10, y: 20 };

  it('a new card in the slot under a dragged card moves past it', () => {
    // 끌기 전: 1 at 0, 2 at 120. 2를 230으로 끌고 3을 띄우면 3의 칸(240)이 2와 겹친다.
    const slot = (id: number) => ({ size: card, dragged: id === 2 ? { x: 230, y: 0 } : undefined });
    const at = arrangeTree([view(1, null), view(2, null), view(3, null)], slot, 'down');
    expect(at.get(2)).toEqual({ x: 230, y: 0 });
    expect(at.get(3)).toEqual({ x: 350, y: 0 });
  });

  it('a group that grows into a dragged group pushes it along the lane', () => {
    const at = placeGroups([g(null, { size: { width: 400, height: 50 } }), g(null, { dragged: { x: 300, y: 20 } })], 'down', start);
    expect(at[0]).toEqual({ x: 10, y: 20 });
    expect(at[1]).toEqual({ x: 442, y: 20 });
  });

  it('the box dropped last stays; the box under it moves aside', () => {
    const at = placeGroups([g(null), g(null, { dragged: { x: 10, y: 20 }, pinned: true })], 'down', start);
    expect(at[1]).toEqual({ x: 10, y: 20 });
    expect(at[0]).toEqual({ x: 142, y: 20 });
  });

  it('wider cards after a view switch push the dragged card that they now cover', () => {
    const wide = { width: 600, height: 400 };
    const slot = (id: number) => ({ size: wide, dragged: id === 2 ? { x: 300, y: 0 } : undefined });
    const at = arrangeTree([view(1, null), view(2, null)], slot, 'down');
    expect(at.get(1)).toEqual({ x: 0, y: 0 });
    expect(at.get(2)).toEqual({ x: 620, y: 0 });
  });

  it('a group that moves in the group tree does not land on a dragged group', () => {
    // 1이 0의 자식 묶음이 되어 (10, 102)로 간다. 2는 그 자리로 끌려 있었다.
    const at = placeGroups([g(null), g(0), g(null, { dragged: { x: 10, y: 102 } })], 'down', start);
    expect(at[0]).toEqual({ x: 10, y: 20 });
    expect(at[1]).toEqual({ x: 10, y: 102 });
    expect(at[2]).toEqual({ x: 142, y: 102 });
  });

  it('right pushes along y', () => {
    const at = placeGroups([g(null, { size: { width: 100, height: 200 } }), g(null, { dragged: { x: 10, y: 100 } })], 'right', start);
    expect(at[1]).toEqual({ x: 10, y: 252 });
  });
});
