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
 * 카드와 묶음이 함께 쓰는 나무 배치(왼쪽 위가 0,0). 자식은 부모 다음 단계(나무가 뻗는 쪽)에 놓고, 부모는 바로 아래 자식들의 가운데에 둔다.
 * 뿌리는 `items` 순서대로 첫 단계에 옆으로 늘어선다. 옆 나무와는 두 나무가 함께 있는 단계마다 틈을 지킨다. 단계의 크기는 그 단계에서 가장 큰 상자라서 크기가 다른 상자도 겹치지 않는다.
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
  const depths = new Map<number, number>();
  const depthSizes: number[] = [];
  // 한 상자와 그 자손의 배치. `at`은 가로지르는 축의 시작 자리, `lo`·`hi`는 단계마다(0이 그 상자) 차지한 양 끝이다.
  type Sub = { at: Map<number, number>; lo: number[]; hi: number[] };
  const shifted = (sub: Sub, by: number): Sub => ({ at: new Map([...sub.at].map(([id, x]) => [id, x + by])), lo: sub.lo.map((x) => x + by), hi: sub.hi.map((x) => x + by) });
  // 형제 나무를 차례로 오른쪽에 붙인다. 두 나무가 함께 있는 단계에서만 `gap.lane`을 지킨다(2026-10-21 사람 지시 "공간낭비 해결방안 마련하라", 목업 docs/mockups/repo-drag.html 안 A).
  const pack = (subs: readonly Sub[]): Sub | null =>
    subs.reduce<Sub | null>((acc, sub) => {
      if (!acc) return sub;
      const shared = Math.min(acc.lo.length, sub.lo.length);
      let by = -Infinity;
      for (let d = 0; d < shared; d++) by = Math.max(by, acc.hi[d] + gap.lane - sub.lo[d]);
      const next = shifted(sub, by);
      const depth = Math.max(acc.lo.length, next.lo.length);
      const pick = (a: number[], b: number[], f: (x: number, y: number) => number) => Array.from({ length: depth }, (_, d) => (a[d] === undefined ? b[d] : b[d] === undefined ? a[d] : f(a[d], b[d])));
      return { at: new Map([...acc.at, ...next.at]), lo: pick(acc.lo, next.lo, Math.min), hi: pick(acc.hi, next.hi, Math.max) };
    }, null);
  const lay = (id: number, depth: number): Sub => {
    const size = byId.get(id)!.size;
    depths.set(id, depth);
    depthSizes[depth] = Math.max(depthSizes[depth] ?? 0, along(size));
    const own = across(size);
    const row = pack(kidsOf(id).map((k) => lay(k, depth + 1)));
    // 부모는 바로 아래 단계 자식들이 차지한 폭의 가운데에 선다.
    const x = row ? (row.lo[0] + row.hi[0]) / 2 - own / 2 : 0;
    return { at: new Map([...(row?.at ?? []), [id, x]]), lo: [x, ...(row?.lo ?? [])], hi: [x + own, ...(row?.hi ?? [])] };
  };
  const all = pack(kidsOf(null).map((root) => lay(root, 0)));
  const min = all ? Math.min(...all.lo) : 0;
  const levels = offsets(depthSizes, gap.depth);
  const reversed = direction === 'up' || direction === 'left';
  return new Map(
    items.map(({ id }) => {
      const d = depths.get(id)!;
      const a = reversed ? levels.total - levels.at[d] - depthSizes[d] : levels.at[d];
      const c = all!.at.get(id)! - min;
      return [id, vertical ? { x: c, y: a } : { x: a, y: c }];
    }),
  );
}

/** 카드 틈. 단계 틈으로 간선이 지나간다. */
const CARD_GAP: TreeGap = { depth: 46, lane: 20 };

/**
 * 카드 위치(묶음 안 카드 자리의 왼쪽 위 기준). `size`는 잰 카드 크기다.
 * 뿌리(사람이 띄운 노드, 부모가 다른 묶음에 있는 노드)도 같은 레벨이라 첫 단계에 옆으로 늘어선다
 * (2026-10-20 사람 지시 "정렬방식은 같은레벨에 있으면 수평으로 확장되게 해줘").
 * 형제는 만든 순서다.
 */
export function arrangeTree(views: readonly NodeView[], size: (id: NodeId) => Size, direction: ArrangeDirection): Map<NodeId, Point> {
  const ids = new Set(views.map((v) => v.id));
  const sorted = [...views].sort((a, b) => a.id - b.id);
  // 부모를 지우면 `removed` 뒤에 자식의 새 상태가 온다. 그 사이에는 그래프에 없는 부모를 가리키므로 뿌리로 놓는다.
  const items = sorted.map((v) => ({ id: v.id, parent: v.parent !== null && ids.has(v.parent) ? v.parent : null, size: size(v.id) }));
  return layoutTree(items, direction, CARD_GAP);
}

/** 옆 묶음과의 틈(목업 docs/mockups/card-view.html 2번). */
const GROUP_GAP = 32;

export type Box = { at: Point; size: Size };

/**
 * 묶음 자리 계산의 입력. `parent`는 부모 묶음의 번호(`groups` 안)이고, 없으면 뿌리 묶음이다. 순환 없는 나무여야 한다(`groupParents`).
 * 형제는 `groups` 순서다.
 */
export type GroupSlot = { size: Size; parent: number | null };

/**
 * 묶음 자리(사용자 요청 2026-10-20 "자동정렬하면 … 트리 방식으로 정렬"). 묶음도 카드와 같은 나무 배치다.
 * 부모 묶음이 없는 뿌리 묶음은 옆으로 늘어선다. 단계와 틈은 잰 묶음 크기와 `GROUP_GAP`이다. 돌려주는 값은 묶음 상자의 왼쪽 위다.
 */
export function placeGroups(groups: readonly GroupSlot[], direction: ArrangeDirection, start: Point): Point[] {
  const items = groups.map((g, i) => ({ id: i, parent: g.parent, size: g.size }));
  const tree = layoutTree(items, direction, { depth: GROUP_GAP, lane: GROUP_GAP });
  return groups.map((_, i) => ({ x: start.x + tree.get(i)!.x, y: start.y + tree.get(i)!.y }));
}

/** 끄는 상자의 형제 하나. `key`는 부르는 쪽이 붙인 상자 이름이다. */
export type Sibling = Box & { key: string };

/** 자유 배치에서 상자 사이 틈(목업 docs/mockups/repo-drag.html v4 ②). */
export const FREE_GAP = 32;

/** 두 상자가 겹치는가. 맞닿기만 하면 겹치지 않는다. */
export function overlaps(a: Box, b: Box): boolean {
  return a.at.x < b.at.x + b.size.width && b.at.x < a.at.x + a.size.width && a.at.y < b.at.y + b.size.height && b.at.y < a.at.y + a.size.height;
}

/** 비키는 쪽. 거리가 같으면 이 순서로 고른다(2026-10-21 총괄 제안, 사람 승인 v4). */
const PUSH_SIDES = ['right', 'down', 'left', 'up'] as const;
type PushSide = (typeof PUSH_SIDES)[number];

/** `box`가 `by`에서 `gap`을 두고 `side` 쪽으로 비키는 거리. */
function clearance(box: Box, by: Box, side: PushSide, gap: number): number {
  switch (side) {
    case 'right':
      return by.at.x + by.size.width + gap - box.at.x;
    case 'down':
      return by.at.y + by.size.height + gap - box.at.y;
    case 'left':
      return box.at.x + box.size.width - (by.at.x - gap);
    case 'up':
      return box.at.y + box.size.height - (by.at.y - gap);
  }
}

function moved(box: Box, side: PushSide, distance: number): Box {
  const d = { right: [distance, 0], down: [0, distance], left: [-distance, 0], up: [0, -distance] }[side];
  return { at: { x: box.at.x + d[0], y: box.at.y + d[1] }, size: box.size };
}

/**
 * 사람 결정(2026-10-21) "겹치는 기존 요소는 상,하,좌,우 중 가장 비켜주는 거리가 짧은 쪽으로 이동".
 * `targets`의 형제가 `mover`에서 네 쪽 가운데 가장 적게 움직이는 쪽으로 `gap`을 두고 비킨다. `mover`는 움직이지 않는다.
 * 비킨 상자가 다른 형제와 겹치면 그 형제도 같은 쪽으로 민다(연쇄). 쪽을 바꾸지 않으므로 끝난다. 움직인 형제의 새 자리만 돌려준다.
 */
export function push(siblings: readonly Sibling[], mover: Box, targets: readonly string[], gap: number): Map<string, Point> {
  const now = new Map(siblings.map((s) => [s.key, s as Box]));
  const out = new Map<string, Point>();
  for (const key of targets) {
    const box = now.get(key);
    if (!box || !overlaps(box, mover)) continue;
    const side = PUSH_SIDES.reduce((best, s) => (clearance(box, mover, s, gap) < clearance(box, mover, best, gap) ? s : best));
    const queue: [string, Box][] = [[key, moved(box, side, clearance(box, mover, side, gap))]];
    while (queue.length > 0) {
      const [k, b] = queue.shift()!;
      now.set(k, b);
      out.set(k, b.at);
      for (const [other, o] of now) {
        if (other !== k && overlaps(o, b)) queue.push([other, moved(o, side, clearance(o, b, side, gap))]);
      }
    }
  }
  return out;
}

/** 끄는 상자 중심이 상자 중심선의 어느 쪽에 있는지(-1, 0, 1). `crossed`에 넘긴다. */
export type CenterSide = { x: number; y: number };

export function centerSide(dragged: Box, box: Box): CenterSide {
  const c = (b: Box) => ({ x: b.at.x + b.size.width / 2, y: b.at.y + b.size.height / 2 });
  const [d, o] = [c(dragged), c(box)];
  return { x: Math.sign(d.x - o.x), y: Math.sign(d.y - o.y) };
}

/** 사람 결정(2026-10-21) "밀려나는 타이밍은 사람이 드래그 잡은게 중앙선 넘엇을경우". 겹치기 전 쪽(`before`)과 지금 쪽이 가로나 세로에서 다르면 넘었다. */
export function crossed(before: CenterSide, now: CenterSide): boolean {
  return before.x !== now.x || before.y !== now.y;
}

/** 자리를 저장하지 않은 새 상자의 자리. `box`가 `others`와 겹치면 겹치지 않을 때까지 겹친 상자 오른쪽으로 `gap`을 두고 옮긴다. */
export function freeSpot(box: Box, others: readonly Box[], gap: number): Point {
  let at = box.at;
  for (let hit = others.find((o) => overlaps({ at, size: box.size }, o)); hit; hit = others.find((o) => overlaps({ at, size: box.size }, o))) {
    at = { x: hit.at.x + hit.size.width + gap, y: at.y };
  }
  return at;
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
 * 아니면(다른 묶음의 자식) 두 카드 사이 빈 틈이 더 큰 축을 따라 서로 마주 보는 변이다.
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
