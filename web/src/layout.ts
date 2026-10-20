// 묶음 안의 나무 배치. 그래프 화면(React Flow)과 무관한 계산만 둔다.
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';

/** 나무가 뿌리에서 뻗는 쪽(목업 docs/mockups/card-view.html 「정렬 방향」). */
export type ArrangeDirection = 'down' | 'right' | 'up' | 'left';
export const ARRANGE_DIRECTIONS: readonly ArrangeDirection[] = ['down', 'right', 'up', 'left'];

export type Size = { width: number; height: number };
type Point = { x: number; y: number };
/** 나무 배치의 입력 한 칸. `parent`가 null이면 뿌리다. 순환 없는 나무여야 한다. */
type TreeItem = { id: number; parent: number | null; size: Size };
/** `depth`는 부모와 자식 사이, `lane`은 형제 사이 틈이다. */
type TreeGap = { depth: number; lane: number };

/** 단계마다 시작 자리. 단계의 크기는 그 단계에서 가장 큰 상자다. */
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
 * 카드와 묶음이 함께 쓰는 나무 배치(왼쪽 위가 0,0). 자식은 부모 다음 단계(나무가 뻗는 쪽)에 놓고, 부모는 자식들이 차지한 폭의 가운데에 둔다.
 * 뿌리는 `items` 순서대로 첫 단계에 옆으로 늘어선다. 단계의 크기는 그 단계에서 가장 큰 상자라서 크기가 다른 상자도 겹치지 않는다.
 * 위로·왼쪽으로는 아래로·오른쪽으로 배치를 뒤집는다.
 */
function layoutTree(items: readonly TreeItem[], direction: ArrangeDirection, gap: TreeGap): Map<number, Point> {
  const vertical = direction === 'down' || direction === 'up';
  const along = (size: Size) => (vertical ? size.height : size.width);
  const across = (size: Size) => (vertical ? size.width : size.height);
  const byId = new Map(items.map((item) => [item.id, item]));
  const kids = new Map<number | null, number[]>();
  for (const item of items) kids.set(item.parent, [...(kids.get(item.parent) ?? []), item.id]);
  const kidsOf = (id: number | null) => kids.get(id) ?? [];
  // 한 상자와 그 자손이 가로질러 차지하는 폭.
  const spans = new Map<number, number>();
  const spanOf = (id: number): number => {
    if (!spans.has(id)) spans.set(id, Math.max(across(byId.get(id)!.size), below(id)));
    return spans.get(id)!;
  };
  const below = (id: number) => kidsOf(id).reduce((sum, k) => sum + spanOf(k), 0) + gap.lane * Math.max(0, kidsOf(id).length - 1);
  const depths = new Map<number, number>();
  const crossAt = new Map<number, number>();
  const depthSizes: number[] = [];
  const place = (id: number, from: number, depth: number) => {
    depths.set(id, depth);
    depthSizes[depth] = Math.max(depthSizes[depth] ?? 0, along(byId.get(id)!.size));
    crossAt.set(id, from + (spanOf(id) - across(byId.get(id)!.size)) / 2);
    let next = from + (spanOf(id) - below(id)) / 2;
    for (const k of kidsOf(id)) {
      place(k, next, depth + 1);
      next += spanOf(k) + gap.lane;
    }
  };
  let nextRoot = 0;
  for (const root of kidsOf(null)) {
    place(root, nextRoot, 0);
    nextRoot += spanOf(root) + gap.lane;
  }
  const levels = offsets(depthSizes, gap.depth);
  const reversed = direction === 'up' || direction === 'left';
  return new Map(
    items.map(({ id }) => {
      const d = depths.get(id)!;
      const a = reversed ? levels.total - levels.at[d] - depthSizes[d] : levels.at[d];
      return [id, vertical ? { x: crossAt.get(id)!, y: a } : { x: a, y: crossAt.get(id)! }];
    }),
  );
}

/** 카드 틈. 단계 틈으로 간선이 지나간다. */
const CARD_GAP: TreeGap = { depth: 46, lane: 20 };

/**
 * 사람이 끌 수 있는 상자(카드·묶음) 하나의 입력. `dragged`는 사람이 끈 자리, `pinned`는 사람이 마지막에 놓은 상자다(겹쳐도 밀리지 않는다).
 * 끈 자리의 좌표는 배치 결과와 같은 기준이다.
 */
export type CardSlot = { size: Size; dragged?: Point; pinned?: boolean };

/**
 * 카드 위치(묶음 안 카드 자리의 왼쪽 위 기준). `slot`은 잰 카드 크기와 끈 자리다.
 * 뿌리(사람이 띄운 노드, 부모가 다른 묶음에 있는 노드)도 같은 레벨이라 첫 단계에 옆으로 늘어선다
 * (2026-10-20 사람 지시 "정렬방식은 같은레벨에 있으면 수평으로 확장되게 해줘"). 형제는 만든 순서다.
 * 끈 카드는 끈 자리에 두고, 그 뒤 겹친 카드를 `untangle`로 민다.
 */
export function arrangeTree(views: readonly NodeView[], slot: (id: NodeId) => CardSlot, direction: ArrangeDirection): Map<NodeId, Point> {
  const ids = new Set(views.map((v) => v.id));
  const sorted = [...views].sort((a, b) => a.id - b.id);
  const slots = sorted.map((v) => slot(v.id));
  // 부모를 지우면 `removed` 뒤에 자식의 새 상태가 온다. 그 사이에는 그래프에 없는 부모를 가리키므로 뿌리로 놓는다.
  const items = sorted.map((v, i) => ({ id: v.id, parent: v.parent !== null && ids.has(v.parent) ? v.parent : null, size: slots[i].size }));
  const tree = layoutTree(items, direction, CARD_GAP);
  const at = untangle(
    slots.map((s, i) => ({ ...s, at: s.dragged ?? tree.get(sorted[i].id)! })),
    direction,
    CARD_GAP.lane,
  );
  return new Map(sorted.map((v, i) => [v.id, at[i]]));
}

/** 옆 묶음과의 틈(목업 docs/mockups/card-view.html 2번). */
const GROUP_GAP = 32;

export type Box = { at: Point; size: Size };

/**
 * 묶음 자리 계산의 입력. `parent`는 부모 묶음의 번호(`groups` 안)이고, 없으면 뿌리 묶음이다. 순환 없는 나무여야 한다(`groupParents`).
 * `lead`는 카드를 왼쪽 테두리 밖으로 끌어 묶음이 자리 왼쪽으로 넓어진 폭이다. `size`는 그 폭을 넣은 크기이고, 끈 자리는 넓어지기 전 왼쪽 위다.
 */
export type GroupSlot = CardSlot & { parent: number | null; lead: number };

/**
 * 묶음 자리(사용자 요청 2026-10-20 "자동정렬하면 … 트리 방식으로 정렬"). 묶음도 카드와 같은 나무 배치다.
 * 부모 묶음이 없는 뿌리 묶음은 `groups` 순서대로 옆으로 늘어선다. 단계와 틈은 잰 묶음 크기와 `GROUP_GAP`이다.
 * 사람이 끈 묶음(`dragged`)은 그 자리에 두고, 그 뒤 겹친 묶음을 `untangle`로 민다.
 * 돌려주는 값은 묶음 상자의 왼쪽 위다. 자리보다 `lead`만큼 왼쪽이다.
 */
export function placeGroups(groups: readonly GroupSlot[], direction: ArrangeDirection, start: Point): Point[] {
  const items = groups.map((g, i) => ({ id: i, parent: g.parent, size: g.size }));
  const tree = layoutTree(items, direction, { depth: GROUP_GAP, lane: GROUP_GAP });
  return untangle(
    groups.map((g, i) => {
      const at = g.dragged ?? { x: start.x + tree.get(i)!.x, y: start.y + tree.get(i)!.y };
      return { ...g, at: { x: at.x - g.lead, y: at.y } };
    }),
    direction,
    GROUP_GAP,
  );
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

const overlaps = (a: Box, b: Box) =>
  a.at.x < b.at.x + b.size.width && b.at.x < a.at.x + a.size.width && a.at.y < b.at.y + b.size.height && b.at.y < a.at.y + a.size.height;

/**
 * 겹친 상자를 형제가 늘어서는 쪽(아래·위로 정렬이면 오른쪽, 오른쪽·왼쪽으로면 아래)으로 민다(목업 docs/mockups/overlap.html A안).
 * 마지막에 놓은 상자를 먼저 두고, 나머지는 그 축에서 앞선 상자부터 둔다. 앞에 둔 상자와 겹치면 그 상자 너머 `lane` 틈 자리로 민다.
 * 겹침이 없으면 아무것도 옮기지 않는다. 민 거리는 기록하지 않으므로 겹침이 사라지면 원래 자리로 돌아온다.
 */
function untangle(boxes: readonly (Box & { pinned?: boolean })[], direction: ArrangeDirection, lane: number): Point[] {
  const vertical = direction === 'down' || direction === 'up';
  const laneOf = (p: Point) => (vertical ? p.x : p.y);
  const alongOf = (p: Point) => (vertical ? p.y : p.x);
  const order = boxes
    .map((_, i) => i)
    .sort((a, b) => {
      const [p, q] = [boxes[a], boxes[b]];
      return Number(!!q.pinned) - Number(!!p.pinned) || laneOf(p.at) - laneOf(q.at) || alongOf(p.at) - alongOf(q.at) || a - b;
    });
  const out: Point[] = boxes.map((b) => b.at);
  const placed: Box[] = [];
  for (const i of order) {
    const { size } = boxes[i];
    const hit = (at: Point) => placed.find((p) => overlaps(p, { at, size }));
    let at = boxes[i].at;
    // 앞에 둔 상자 너머로만 옮기므로 같은 상자에 다시 걸리지 않는다. 그래서 끝난다.
    for (let h = hit(at); h; h = hit(at)) at = vertical ? { x: h.at.x + h.size.width + lane, y: at.y } : { x: at.x, y: h.at.y + h.size.height + lane };
    out[i] = at;
    placed.push({ at, size });
  }
  return out;
}
