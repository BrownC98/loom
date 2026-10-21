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
 * 자식은 부모 다음 단계에서 시작한다. `rows`는 차지하는 단계 수다(카드는 1, 자식 묶음 줄 자리는 그 줄까지).
 */
type TreeItem = { id: number; parent: number | null; size: Size; rows: number };
/** `level`은 단계 사이 틈, `lane`은 형제 사이 틈이다. */
type TreeGap = { level: number; lane: number };

/**
 * 카드 나무 배치(왼쪽 위가 0,0). 자식은 부모 다음 단계(나무가 뻗는 쪽)에 놓고, 부모는 바로 아래 자식들의 가운데에 둔다.
 * 형제와 뿌리는 `items` 순서대로 옆으로 늘어선다. 옆 나무와는 두 나무가 함께 있는 단계마다 틈을 지킨다.
 * 단계의 크기는 그 단계에서 가장 큰 칸이고, 단계 사이는 늘 `gap.level`이다(2026-10-21 사람 피드백, 목업 docs/mockups/arrange-wide.html 「줄 사이 틈」).
 * 위로·왼쪽으로는 아래로·오른쪽으로 배치를 뒤집는다.
 */
function layoutTree(items: readonly TreeItem[], direction: ArrangeDirection, gap: TreeGap): Map<number, Point> {
  const vertical = direction === 'down' || direction === 'up';
  const across = (size: Size) => (vertical ? size.width : size.height);
  const along = (size: Size) => (vertical ? size.height : size.width);
  const byId = new Map(items.map((item) => [item.id, item]));
  const kids = new Map<number | null, number[]>();
  for (const item of items) kids.set(item.parent, [...(kids.get(item.parent) ?? []), item.id]);
  const kidsOf = (id: number | null) => kids.get(id) ?? [];
  const levels = new Map<number, number>();
  // 한 칸과 그 자손의 배치. `at`은 가로지르는 축의 시작 자리, `lo`·`hi`는 단계마다 차지한 양 끝이다.
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
    const row = pack(kidsOf(id).map((k) => lay(k, level + 1)));
    // 부모는 바로 아래 단계 자식들이 차지한 폭의 가운데에 선다.
    const x = row ? (row.lo.get(level + 1)! + row.hi.get(level + 1)!) / 2 - own / 2 : 0;
    const mine = Array.from({ length: item.rows }, (_, r) => level + r);
    return {
      at: new Map([...(row?.at ?? []), [id, x]]),
      lo: merged(row?.lo ?? new Map(), new Map(mine.map((l) => [l, x])), Math.min),
      hi: merged(row?.hi ?? new Map(), new Map(mine.map((l) => [l, x + own])), Math.max),
    };
  };
  const all = pack(kidsOf(null).map((root) => lay(root, 0)));
  const min = all ? Math.min(...all.lo.values()) : 0;
  // 단계마다 시작 자리. 단계 크기는 그 단계에서 가장 큰 칸이다.
  const last = Math.max(0, ...items.map((item) => levels.get(item.id)!));
  const size = Array.from({ length: last + 1 }, (_, l) => Math.max(0, ...items.filter((item) => levels.get(item.id) === l).map((item) => along(item.size))));
  const start = size.reduce<number[]>((acc, _, l) => [...acc, l === 0 ? 0 : acc[l - 1] + size[l - 1] + gap.level], []);
  const end = start[last] + size[last];
  const reversed = direction === 'up' || direction === 'left';
  return new Map(
    items.map(({ id, size: s }) => {
      const level = levels.get(id)!;
      const a = reversed ? end - start[level] - along(s) : start[level];
      const c = all!.at.get(id)! - min;
      return [id, vertical ? { x: c, y: a } : { x: a, y: c }];
    }),
  );
}

/** 카드 형제 틈. */
const CARD_LANE = 20;

/**
 * 묶음 하나의 카드 위치(카드 자리의 왼쪽 위 기준). `size`는 잰 카드 크기, `gap`은 카드 줄 사이 틈이다.
 * 뿌리(사람이 띄운 노드, 부모가 다른 묶음에 있는 노드)는 첫 단계에 옆으로 늘어선다
 * (2026-10-20 사람 지시 "정렬방식은 같은레벨에 있으면 수평으로 확장되게 해줘").
 * 형제는 `views` 순서다. 부르는 쪽이 지금 화면 순서로 넘긴다(목업 docs/mockups/arrange-wide.html 규칙 4).
 * `bands`는 다른 묶음에 자식이 있는 카드마다 묶음 아래 자식 묶음 줄의 폭이다. 줄은 그 카드의 자식처럼 놓이고,
 * 카드와 줄 사이에는 다른 카드가 서지 않는다(같은 목업 규칙 1·3). 돌려주는 `bands`는 줄의 시작 자리(가로지르는 축)다.
 */
export function arrangeTree(
  views: readonly NodeView[],
  size: (id: NodeId) => Size,
  direction: ArrangeDirection,
  gap: number,
  bands: ReadonlyMap<NodeId, number>,
): { cards: Map<NodeId, Point>; bands: Map<NodeId, number> } {
  const ids = new Set(views.map((v) => v.id));
  // 부모를 지우면 `removed` 뒤에 자식의 새 상태가 온다. 그 사이에는 그래프에 없는 부모를 가리키므로 뿌리로 놓는다.
  const parentOf = (v: NodeView) => (v.parent !== null && ids.has(v.parent) ? v.parent : null);
  const byId = new Map(views.map((v) => [v.id, v]));
  const depth = (v: NodeView): number => {
    const up = parentOf(v);
    return up === null ? 0 : depth(byId.get(up)!) + 1;
  };
  const deepest = Math.max(...views.map(depth));
  const vertical = direction === 'down' || direction === 'up';
  // 줄 자리는 카드 아래 단계부터 묶음 아래 첫 단계까지 차지한다. 번호는 카드 번호와 겹치지 않게 음수다.
  const band = (id: NodeId) => -1 - id;
  const items = [
    ...views.map((v) => ({ id: v.id as number, parent: parentOf(v) as number | null, size: size(v.id), rows: 1 })),
    ...views.flatMap((v) => {
      const width = bands.get(v.id);
      return width === undefined ? [] : [{ id: band(v.id), parent: v.id as number, size: vertical ? { width, height: 0 } : { width: 0, height: width }, rows: deepest + 1 - depth(v) }];
    }),
  ];
  const at = layoutTree(items, direction, { level: gap, lane: CARD_LANE });
  return {
    cards: new Map(views.map((v) => [v.id, at.get(v.id)!])),
    bands: new Map([...bands.keys()].map((id) => [id, vertical ? at.get(band(id))!.x : at.get(band(id))!.y])),
  };
}

/** 옆 묶음과의 틈(목업 docs/mockups/card-view.html 2번). */
export const GROUP_GAP = 32;

export type Box = { at: Point; size: Size };

/**
 * 묶음 자리 계산의 입력. `parent`는 부모 묶음의 번호(`groups` 안)이고, 없으면 뿌리 묶음이다. 순환 없는 나무여야 한다(`groupParents`).
 * `card`는 부모 묶음 안의 부모 카드다(`attachCards`). 형제는 `groups` 순서다.
 */
export type GroupSlot = { parent: number | null; card: NodeId | null };

/** 묶음 하나를 놓은 결과. `band`는 그 카드 아래 자식 묶음 줄 자리의 시작을 묶음 상자 시작에서 가로지르는 축으로 잰 값이다. */
export type LaidGroup = { size: Size; band: (card: NodeId) => number };

/**
 * 묶음 자리(목업 docs/mockups/arrange-wide.html ②). 다른 묶음의 자식은 부모 묶음 아래 줄에 선다. 줄 자리는 `lay`가 부모 카드의 자식으로 잡는다.
 * 자식부터 놓는다. `lay(i, bands)`는 묶음 `i`의 카드를 놓고 크기를 돌려준다. `bands`는 카드마다 그 아래 자식 묶음 줄의 폭에 `GROUP_GAP`을 더한 값이다.
 * 줄은 그 자리 가운데에 선다. 그래서 `arrangeTree`가 줄 자리를 카드 틈으로 붙여도 두 카드의 줄 사이는 `GROUP_GAP`보다 넓다.
 * 묶음 깊이가 같으면 같은 줄이다. 줄의 크기는 그 줄에서 가장 큰 묶음이고, 줄 사이와 옆 묶음 사이는 `GROUP_GAP`이다.
 * 돌려주는 값은 묶음 상자의 왼쪽 위다(가장 위·왼쪽이 0).
 */
export function placeGroups(groups: readonly GroupSlot[], lay: (i: number, bands: ReadonlyMap<NodeId, number>) => LaidGroup, direction: ArrangeDirection): Point[] {
  const vertical = direction === 'down' || direction === 'up';
  const across = (size: Size) => (vertical ? size.width : size.height);
  const along = (size: Size) => (vertical ? size.height : size.width);
  const kids = groups.map((_, i) => groups.flatMap((g, k) => (g.parent === i ? [k] : [])));
  const laid: LaidGroup[] = [];
  // 묶음과 그 자손 줄이 가로지르는 축으로 차지하는 양 끝(묶음 상자 시작 기준)과 깊이.
  const lo: number[] = [];
  const hi: number[] = [];
  const depth: number[] = [];
  const cards = (i: number) => [...new Set(kids[i].map((k) => groups[k].card!))];
  const width = (k: number) => hi[k] - lo[k];
  const bandOf = (i: number, card: NodeId) => kids[i].filter((k) => groups[k].card === card);
  const bandWidth = (i: number, card: NodeId) => bandOf(i, card).reduce((sum, k, n) => sum + width(k) + (n > 0 ? GROUP_GAP : 0), 0);
  const measure = (i: number, d: number) => {
    depth[i] = d;
    for (const k of kids[i]) measure(k, d + 1);
    laid[i] = lay(i, new Map(cards(i).map((card) => [card, bandWidth(i, card) + GROUP_GAP])));
    lo[i] = 0;
    hi[i] = across(laid[i].size);
    for (const card of cards(i)) {
      const b = laid[i].band(card) + GROUP_GAP / 2;
      lo[i] = Math.min(lo[i], b);
      hi[i] = Math.max(hi[i], b + bandWidth(i, card));
    }
  };
  const roots = groups.flatMap((g, i) => (g.parent === null ? [i] : []));
  for (const r of roots) measure(r, 0);
  const cross: number[] = [];
  const put = (i: number, x: number) => {
    cross[i] = x;
    for (const card of cards(i)) {
      let cursor = x + laid[i].band(card) + GROUP_GAP / 2;
      for (const k of bandOf(i, card)) {
        put(k, cursor - lo[k]);
        cursor += width(k) + GROUP_GAP;
      }
    }
  };
  roots.reduce((cursor, r) => {
    put(r, cursor - lo[r]);
    return cursor + width(r) + GROUP_GAP;
  }, 0);
  const deepest = Math.max(0, ...depth);
  const size = Array.from({ length: deepest + 1 }, (_, d) => Math.max(0, ...groups.flatMap((_, i) => (depth[i] === d ? [along(laid[i].size)] : []))));
  const start = size.reduce<number[]>((acc, _, d) => [...acc, d === 0 ? 0 : acc[d - 1] + size[d - 1] + GROUP_GAP], []);
  const end = start[deepest] + size[deepest];
  const reversed = direction === 'up' || direction === 'left';
  return groups.map((_, i) => {
    const a = reversed ? end - start[depth[i]] - along(laid[i].size) : start[depth[i]];
    return vertical ? { x: cross[i], y: a } : { x: a, y: cross[i] };
  });
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

/** `before`에도 있던 상자 가운데 폭이나 높이가 커진 상자의 이름. 새 상자는 `freeSpot`이 자리를 잡으므로 넣지 않는다. */
export function grew(before: ReadonlyMap<string, Size>, now: ReadonlyMap<string, Size>): string[] {
  return [...now].flatMap(([key, size]) => {
    const was = before.get(key);
    return was && (size.width > was.width || size.height > was.height) ? [key] : [];
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
