// 묶음 안의 나무 배치. 그래프 화면(React Flow)과 무관한 계산만 둔다.
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';

/** 나무가 뿌리에서 뻗는 쪽(목업 docs/mockups/card-view.html 「정렬 방향」). */
export type ArrangeDirection = 'down' | 'right' | 'up' | 'left';
export const ARRANGE_DIRECTIONS: readonly ArrangeDirection[] = ['down', 'right', 'up', 'left'];

export type Size = { width: number; height: number };
type Point = { x: number; y: number };
/**
 * 나무 배치의 입력 한 칸. `parent`가 null이면 뿌리다. 순환 없는 나무여야 한다.
 * `rows`는 차지하는 단계 수, `start`는 부모 첫 단계에서 이 상자 첫 단계까지의 단계 수다(카드는 둘 다 1).
 */
type TreeItem = { id: number; parent: number | null; size: Size; rows: number; start: number };
/** `step`은 한 단계의 크기(모든 단계가 같다), `lane`은 형제 사이 틈이다. */
type TreeGap = { step: number; lane: number };

/**
 * 카드와 묶음이 함께 쓰는 나무 배치(왼쪽 위가 0,0). 자식은 부모 다음 단계(나무가 뻗는 쪽)에 놓고, 부모는 바로 아래 자식들의 가운데에 둔다.
 * 자식이 부모가 차지한 단계에서 시작하면(`start` < 부모 `rows`) 부모 옆에 선다(목업 docs/mockups/same-worktree.html 안 B).
 * 뿌리는 `items` 순서대로 첫 단계에 옆으로 늘어선다. 옆 나무와는 두 나무가 함께 있는 단계마다 틈을 지킨다.
 * 위로·왼쪽으로는 아래로·오른쪽으로 배치를 뒤집는다.
 */
function layoutTree(items: readonly TreeItem[], direction: ArrangeDirection, gap: TreeGap): Map<number, Point> {
  const vertical = direction === 'down' || direction === 'up';
  const across = (size: Size) => (vertical ? size.width : size.height);
  const byId = new Map(items.map((item) => [item.id, item]));
  const kids = new Map<number | null, number[]>();
  for (const item of items) kids.set(item.parent, [...(kids.get(item.parent) ?? []), item.id]);
  const kidsOf = (id: number | null) => kids.get(id) ?? [];
  const levels = new Map<number, number>();
  // 한 상자와 그 자손의 배치. `at`은 가로지르는 축의 시작 자리, `lo`·`hi`는 단계마다 차지한 양 끝이다.
  type Sub = { at: Map<number, number>; lo: Map<number, number>; hi: Map<number, number> };
  const moved = (m: Map<number, number>, by: number) => new Map([...m].map(([k, x]) => [k, x + by]));
  const merged = (a: Map<number, number>, b: Map<number, number>, f: (x: number, y: number) => number) => {
    const out = new Map(a);
    for (const [k, x] of b) out.set(k, out.has(k) ? f(out.get(k)!, x) : x);
    return out;
  };
  // 형제 나무를 차례로 오른쪽에 붙인다. 두 나무가 함께 있는 단계에서만 `gap.lane`을 지킨다(2026-10-21 사람 지시 "공간낭비 해결방안 마련하라", 목업 docs/mockups/repo-drag.html 안 A).
  const pack = (subs: readonly Sub[]): Sub | null =>
    subs.reduce<Sub | null>((acc, sub) => {
      if (!acc) return sub;
      let by = -Infinity;
      for (const [level, lo] of sub.lo) if (acc.hi.has(level)) by = Math.max(by, acc.hi.get(level)! + gap.lane - lo);
      if (by === -Infinity) by = Math.max(...acc.hi.values()) + gap.lane - Math.min(...sub.lo.values());
      return { at: new Map([...acc.at, ...moved(sub.at, by)]), lo: merged(acc.lo, moved(sub.lo, by), Math.min), hi: merged(acc.hi, moved(sub.hi, by), Math.max) };
    }, null);
  const lay = (id: number, level: number): Sub => {
    const item = byId.get(id)!;
    levels.set(id, level);
    const own = across(item.size);
    const below = kidsOf(id).filter((k) => byId.get(k)!.start >= item.rows);
    const beside = kidsOf(id).filter((k) => byId.get(k)!.start < item.rows);
    const row = pack(below.map((k) => lay(k, level + byId.get(k)!.start)));
    // 부모는 바로 아래 단계 자식들이 차지한 폭의 가운데에 선다.
    const first = Math.min(...below.map((k) => levels.get(k)!));
    const x = row ? (row.lo.get(first)! + row.hi.get(first)!) / 2 - own / 2 : 0;
    const mine = Array.from({ length: item.rows }, (_, r) => level + r);
    const self: Sub = {
      at: new Map([...(row?.at ?? []), [id, x]]),
      lo: merged(row?.lo ?? new Map(), new Map(mine.map((l) => [l, x])), Math.min),
      hi: merged(row?.hi ?? new Map(), new Map(mine.map((l) => [l, x + own])), Math.max),
    };
    return pack([self, ...beside.map((k) => lay(k, level + byId.get(k)!.start))])!;
  };
  const all = pack(kidsOf(null).map((root) => lay(root, 0)));
  const min = all ? Math.min(...all.lo.values()) : 0;
  const last = Math.max(...items.map((item) => levels.get(item.id)! + item.rows - 1));
  const reversed = direction === 'up' || direction === 'left';
  return new Map(
    items.map(({ id, rows }) => {
      const level = levels.get(id)!;
      const a = (reversed ? last - (level + rows - 1) : level) * gap.step;
      const c = all!.at.get(id)! - min;
      return [id, vertical ? { x: c, y: a } : { x: a, y: c }];
    }),
  );
}

/** 카드 형제 틈. */
const CARD_LANE = 20;

/**
 * 카드 위치(묶음 안 카드 자리의 왼쪽 위 기준). `size`는 잰 카드 크기, `step`은 한 단계의 크기다.
 * 저장소의 모든 묶음이 같은 `step`을 써서 깊이가 같은 카드는 묶음과 관계없이 같은 줄에 선다(목업 docs/mockups/same-worktree.html 안 B).
 * 뿌리(사람이 띄운 노드, 부모가 다른 묶음에 있는 노드)도 같은 레벨이라 첫 단계에 옆으로 늘어선다
 * (2026-10-20 사람 지시 "정렬방식은 같은레벨에 있으면 수평으로 확장되게 해줘").
 * 형제는 만든 순서다.
 */
export function arrangeTree(views: readonly NodeView[], size: (id: NodeId) => Size, direction: ArrangeDirection, step: number): Map<NodeId, Point> {
  const ids = new Set(views.map((v) => v.id));
  const sorted = [...views].sort((a, b) => a.id - b.id);
  // 부모를 지우면 `removed` 뒤에 자식의 새 상태가 온다. 그 사이에는 그래프에 없는 부모를 가리키므로 뿌리로 놓는다.
  const items = sorted.map((v) => ({ id: v.id, parent: v.parent !== null && ids.has(v.parent) ? v.parent : null, size: size(v.id), rows: 1, start: 1 }));
  return layoutTree(items, direction, { step, lane: CARD_LANE });
}

/** 옆 묶음과의 틈(목업 docs/mockups/card-view.html 2번). */
export const GROUP_GAP = 32;

export type Box = { at: Point; size: Size };

/**
 * 묶음 자리 계산의 입력. `parent`는 부모 묶음의 번호(`groups` 안)이고, 없으면 뿌리 묶음이다. 순환 없는 나무여야 한다(`groupParents`).
 * `start`는 부모 묶음 첫 단계에서 이 묶음 첫 단계까지의 단계 수다(`attachLevels`). 형제는 `groups` 순서다.
 */
export type GroupSlot = { size: Size; parent: number | null; start: number };

/**
 * 묶음 자리(사용자 요청 2026-10-20 "자동정렬하면 … 트리 방식으로 정렬"). 묶음도 카드와 같은 나무 배치다.
 * 묶음은 크기를 덮는 단계 수만큼 차지한다. 한 단계는 `step`이고 묶음 틈은 `GROUP_GAP`이다. 돌려주는 값은 묶음 상자의 왼쪽 위다.
 */
export function placeGroups(groups: readonly GroupSlot[], direction: ArrangeDirection, start: Point, step: number): Point[] {
  const along = (size: Size) => (direction === 'down' || direction === 'up' ? size.height : size.width);
  // k단계 묶음의 크기는 k * step - GROUP_GAP이다. 잰 크기의 소수점 때문에 조금 줄인다.
  const rows = (size: Size) => Math.max(1, Math.ceil((along(size) + GROUP_GAP) / step - 0.01));
  const items = groups.map((g, i) => ({ id: i, parent: g.parent, size: g.size, rows: rows(g.size), start: g.start }));
  const tree = layoutTree(items, direction, { step, lane: GROUP_GAP });
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
