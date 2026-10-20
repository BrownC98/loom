// 묶음 안의 나무 배치. 그래프 화면(React Flow)과 무관한 계산만 둔다.
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';

/** 나무가 뿌리에서 뻗는 쪽(목업 docs/mockups/card-view.html 「정렬 방향」). */
export type ArrangeDirection = 'down' | 'right' | 'up' | 'left';
export const ARRANGE_DIRECTIONS: readonly ArrangeDirection[] = ['down', 'right', 'up', 'left'];

export type Size = { width: number; height: number };
type Point = { x: number; y: number };
/** `depth`는 뿌리에서 몇 단계 아래인지, `lane`은 형제가 늘어서는 칸이다. */
type Slot = { lane: number; depth: number };

/** 부모와 자식 사이 틈. 간선이 지나간다. */
const DEPTH_GAP = 46;
/** 형제 사이 틈. */
const LANE_GAP = 20;

/**
 * 나무 칸: 자식은 부모 다음 단계에, 형제는 다음 칸에 놓는다. 잎마다 새 칸을 쓰고 부모는 첫 자식의 칸에 둔다.
 * 사람이 띄운 노드(뿌리)는 앞 뿌리의 나무 다음 단계에 쌓는다. 형제는 만든 순서다.
 */
function treeSlots(views: readonly NodeView[]): Map<NodeId, Slot> {
  const ids = new Set(views.map((v) => v.id));
  const children = new Map<NodeId | null, NodeView[]>();
  for (const view of [...views].sort((a, b) => a.id - b.id)) {
    // 부모를 지우면 `removed` 뒤에 자식의 새 상태가 온다. 그 사이에는 그래프에 없는 부모를 가리키므로 뿌리로 놓는다.
    const parent = view.parent !== null && ids.has(view.parent) ? view.parent : null;
    children.set(parent, [...(children.get(parent) ?? []), view]);
  }
  const slots = new Map<NodeId, Slot>();
  let firstDepth = 0;
  for (const root of children.get(null) ?? []) {
    let lane = 0;
    let lastDepth = firstDepth;
    const place = (view: NodeView, depth: number) => {
      slots.set(view.id, { lane, depth });
      lastDepth = Math.max(lastDepth, depth);
      const kids = children.get(view.id) ?? [];
      if (kids.length === 0) lane++;
      for (const kid of kids) place(kid, depth + 1);
    };
    place(root, firstDepth);
    firstDepth = lastDepth + 1;
  }
  return slots;
}

/** 칸마다 시작 자리. 칸의 크기는 그 칸에서 가장 큰 카드다. */
function offsets(sizes: readonly number[], gap: number): { at: number[]; total: number } {
  const at: number[] = [];
  let next = 0;
  for (const size of sizes) {
    at.push(next);
    next += size + gap;
  }
  return { at, total: Math.max(0, next - gap) };
}

/**
 * 카드 위치(묶음 안 카드 자리의 왼쪽 위 기준). `size`는 잰 카드 크기다. 단계와 칸의 크기를 잰 크기로 정하므로 크기가 다른 카드도 겹치지 않는다.
 * 위로·왼쪽으로는 아래로·오른쪽으로 배치를 뒤집는다.
 */
export function arrangeTree(views: readonly NodeView[], size: (id: NodeId) => Size, direction: ArrangeDirection): Map<NodeId, Point> {
  const slots = treeSlots(views);
  const vertical = direction === 'down' || direction === 'up';
  const depthSizes: number[] = [];
  const laneSizes: number[] = [];
  for (const [id, { lane, depth }] of slots) {
    const { width, height } = size(id);
    depthSizes[depth] = Math.max(depthSizes[depth] ?? 0, vertical ? height : width);
    laneSizes[lane] = Math.max(laneSizes[lane] ?? 0, vertical ? width : height);
  }
  const depths = offsets(Array.from(depthSizes, (s) => s ?? 0), DEPTH_GAP);
  const lanes = offsets(Array.from(laneSizes, (s) => s ?? 0), LANE_GAP);
  const reversed = direction === 'up' || direction === 'left';
  const positions = new Map<NodeId, Point>();
  for (const [id, { lane, depth }] of slots) {
    const along = reversed ? depths.total - depths.at[depth] - depthSizes[depth] : depths.at[depth];
    const across = lanes.at[lane];
    positions.set(id, vertical ? { x: across, y: along } : { x: along, y: across });
  }
  return positions;
}

/** 옆 묶음과의 틈(목업 docs/mockups/card-view.html 2번). */
const GROUP_GAP = 32;

export type Box = { at: Point; size: Size };

/** 묶음 자리 계산의 입력. `parent`는 부모 묶음의 번호(`groups` 안)이고, 없으면 뿌리 묶음이다. 순환 없는 나무여야 한다(`groupParents`). */
export type GroupSlot = { size: Size; parent: number | null; dragged?: Point };

/**
 * 묶음 자리(사용자 요청 2026-10-20 "자동정렬하면 … 트리 방식으로 정렬"). 묶음을 카드처럼 나무로 놓는다.
 * 자식 묶음은 부모 묶음 다음 단계(나무가 뻗는 쪽)에, 형제는 옆(가로질러)에 놓고, 부모는 자식들이 차지한 폭의 가운데에 둔다.
 * 부모 묶음이 없는 뿌리 묶음은 `groups` 순서대로 옆으로 늘어선다. 단계와 틈은 잰 묶음 크기와 `GROUP_GAP`이다.
 * 사람이 끈 묶음(`dragged`)은 그 자리에 둔다. 다른 묶음의 자리는 바꾸지 않는다.
 */
export function placeGroups(groups: readonly GroupSlot[], direction: ArrangeDirection, start: Point): Point[] {
  const vertical = direction === 'down' || direction === 'up';
  const along = (size: Size) => (vertical ? size.height : size.width);
  const across = (size: Size) => (vertical ? size.width : size.height);
  const parents = groups.map((g) => g.parent);
  const kids = groups.map((_, i) => parents.flatMap((p, k) => (p === i ? [k] : [])));
  const depthOf = (i: number): number => (parents[i] === null ? 0 : depthOf(parents[i]!) + 1);
  const depthSizes: number[] = [];
  groups.forEach((g, i) => {
    const d = depthOf(i);
    depthSizes[d] = Math.max(depthSizes[d] ?? 0, along(g.size));
  });
  const depths = offsets(depthSizes, GROUP_GAP);
  // 한 묶음과 그 자손이 가로질러 차지하는 폭.
  const span = new Map<number, number>();
  const spanOf = (i: number): number => {
    if (!span.has(i)) {
      const below = kids[i].reduce((sum, k) => sum + spanOf(k), 0) + GROUP_GAP * Math.max(0, kids[i].length - 1);
      span.set(i, Math.max(across(groups[i].size), below));
    }
    return span.get(i)!;
  };
  const crossAt = new Map<number, number>();
  const place = (i: number, from: number) => {
    const own = across(groups[i].size);
    crossAt.set(i, from + (spanOf(i) - own) / 2);
    const below = kids[i].reduce((sum, k) => sum + spanOf(k), 0) + GROUP_GAP * Math.max(0, kids[i].length - 1);
    let next = from + (spanOf(i) - below) / 2;
    for (const k of kids[i]) {
      place(k, next);
      next += spanOf(k) + GROUP_GAP;
    }
  };
  let nextRoot = vertical ? start.x : start.y;
  parents.forEach((p, i) => {
    if (p !== null) return;
    place(i, nextRoot);
    nextRoot += spanOf(i) + GROUP_GAP;
  });
  const reversed = direction === 'up' || direction === 'left';
  const origin = vertical ? start.y : start.x;
  return groups.map((g, i) => {
    const d = depthOf(i);
    const a = origin + (reversed ? depths.total - depths.at[d] - depthSizes[d] : depths.at[d]);
    return g.dragged ?? (vertical ? { x: crossAt.get(i)!, y: a } : { x: a, y: crossAt.get(i)! });
  });
}

/** 간선이 붙는 카드의 변. */
export type Side = 'top' | 'right' | 'bottom' | 'left';
export const SIDES: readonly Side[] = ['top', 'right', 'bottom', 'left'];
/** 나무가 뻗는 쪽으로 이은 간선의 변. `parent`는 부모 카드에서 나가는 변, `child`는 자식 카드로 들어오는 변이다. */
export const TREE_SIDES: Record<ArrangeDirection, { parent: Side; child: Side }> = {
  down: { parent: 'bottom', child: 'top' },
  up: { parent: 'top', child: 'bottom' },
  right: { parent: 'right', child: 'left' },
  left: { parent: 'left', child: 'right' },
};

/**
 * 부모와 자식 카드(화면 기준 상자)를 잇는 간선의 변. 자식이 부모보다 나무가 뻗는 쪽에 있으면 `TREE_SIDES[direction]`이다.
 * 아니면(다른 묶음의 자식, 끈 카드) 두 카드 사이 빈 틈이 더 큰 축을 따라 서로 마주 보는 변이다.
 * 중심 거리로 축을 고르면 가로로 겹친 카드(틈이 좁다)에서 간선이 카드 옆 틈을 돌아 나가 꺾인다.
 */
export function edgeSides(parent: Box, child: Box, direction: ArrangeDirection): { parent: Side; child: Side } {
  const ahead: Record<ArrangeDirection, boolean> = {
    down: child.at.y >= parent.at.y + parent.size.height,
    up: child.at.y + child.size.height <= parent.at.y,
    right: child.at.x >= parent.at.x + parent.size.width,
    left: child.at.x + child.size.width <= parent.at.x,
  };
  if (ahead[direction]) return TREE_SIDES[direction];
  const gapRight = child.at.x - (parent.at.x + parent.size.width);
  const gapLeft = parent.at.x - (child.at.x + child.size.width);
  const gapDown = child.at.y - (parent.at.y + parent.size.height);
  const gapUp = parent.at.y - (child.at.y + child.size.height);
  const horizontal = Math.max(gapRight, gapLeft) > Math.max(gapDown, gapUp);
  const toward: ArrangeDirection = horizontal ? (gapRight > gapLeft ? 'right' : 'left') : gapDown > gapUp ? 'down' : 'up';
  return TREE_SIDES[toward];
}
