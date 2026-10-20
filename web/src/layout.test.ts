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

const sized = () => ({ width: 100, height: 50 });

describe('arrangeTree (today: parent at the first child lane)', () => {
  // 1 -> {2, 3}, 2 -> {4}
  const tree = [view(1, null), view(2, 1), view(3, 1), view(4, 2)];

  it('down: child at the next depth, siblings in the next lane, parent at the first child lane', () => {
    const at = arrangeTree(tree, sized, 'down');
    expect(at.get(1)).toEqual({ x: 0, y: 0 });
    expect(at.get(2)).toEqual({ x: 0, y: 96 });
    expect(at.get(4)).toEqual({ x: 0, y: 192 });
    expect(at.get(3)).toEqual({ x: 120, y: 96 });
  });

  it('right: same slots with depth along x and lane along y', () => {
    const at = arrangeTree(tree, sized, 'right');
    expect(at.get(1)).toEqual({ x: 0, y: 0 });
    expect(at.get(2)).toEqual({ x: 146, y: 0 });
    expect(at.get(4)).toEqual({ x: 292, y: 0 });
    expect(at.get(3)).toEqual({ x: 146, y: 70 });
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

  it('depth and lane sizes follow the largest card, so mixed sizes do not overlap', () => {
    const size = (id: number) => (id === 2 ? { width: 200, height: 80 } : sized());
    const at = arrangeTree([view(1, null), view(2, 1), view(3, 1)], size, 'down');
    expect(at.get(2)).toEqual({ x: 0, y: 96 });
    expect(at.get(3)).toEqual({ x: 220, y: 96 });
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
