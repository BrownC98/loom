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

/** 두 상자가 `GROUP_GAP` 안으로 붙거나 겹친다. */
function crowds(a: Box, b: Box): boolean {
  return (
    a.at.x < b.at.x + b.size.width + GROUP_GAP &&
    b.at.x < a.at.x + a.size.width + GROUP_GAP &&
    a.at.y < b.at.y + b.size.height + GROUP_GAP &&
    b.at.y < a.at.y + a.size.height + GROUP_GAP
  );
}

/**
 * 묶음 자리(목업 docs/mockups/card-view.html 2·3번). 나무가 아래·위로 뻗으면 묶음을 가로로, 오른쪽·왼쪽으로 뻗으면 세로로 놓는다.
 * 아래로는 위쪽, 위로는 아래쪽, 오른쪽으로는 왼쪽, 왼쪽으로는 오른쪽을 맞춘다. 순서는 `groups` 순서(왼쪽 목록의 워크트리 순서)다.
 * 사람이 끈 묶음(`dragged`)은 그 자리에 둔다. 앞 묶음과 겹치거나 틈보다 가까우면 줄 방향으로 민다.
 */
export function placeGroups(groups: readonly { size: Size; dragged?: Point }[], direction: ArrangeDirection, start: Point): Point[] {
  const row = direction === 'down' || direction === 'up';
  const crossMax = Math.max(0, ...groups.map((g) => (row ? g.size.height : g.size.width)));
  const placed: Box[] = [];
  let next = row ? start.x : start.y;
  return groups.map(({ size, dragged }) => {
    const cross =
      direction === 'up' ? start.y + crossMax - size.height : direction === 'left' ? start.x + crossMax - size.width : row ? start.y : start.x;
    const box: Box = { at: dragged ?? (row ? { x: next, y: cross } : { x: cross, y: next }), size };
    for (let pushed = true; pushed; ) {
      pushed = false;
      for (const other of placed) {
        if (!crowds(box, other)) continue;
        box.at = row ? { ...box.at, x: other.at.x + other.size.width + GROUP_GAP } : { ...box.at, y: other.at.y + other.size.height + GROUP_GAP };
        pushed = true;
      }
    }
    placed.push(box);
    // 끈 묶음은 다음 묶음의 자리를 바꾸지 않는다. 겹치면 위에서 민다.
    if (!dragged) next = (row ? box.at.x + size.width : box.at.y + size.height) + GROUP_GAP;
    return box.at;
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
