// 묶음 안의 나무 배치. 그래프 화면(React Flow)과 무관한 계산만 둔다.
import type { NodeId } from './gen/NodeId';

/** 나무가 뿌리에서 뻗는 쪽(목업 docs/mockups/card-view.html 「정렬 방향」). */
export type ArrangeDirection = 'down' | 'right' | 'up' | 'left';
export const ARRANGE_DIRECTIONS: readonly ArrangeDirection[] = ['down', 'right', 'up', 'left'];

export type Size = { width: number; height: number };
type Point = { x: number; y: number };
/** 카드 형제 틈. */
const CARD_LANE = 20;

/** 옆 묶음과의 틈(목업 docs/mockups/card-view.html 2번). 상자가 낀 두 상자 사이도 이 틈이다. */
export const GROUP_GAP = 32;

export type Box = { at: Point; size: Size };

/**
 * 묶음 하나의 입력. `parent`는 부모 묶음 번호(`groups` 안), `card`는 그 안의 부모 카드다(`groupParents`, `attachCards`). 둘 다 null이면 맨 위 묶음이다.
 * `members`는 이 묶음의 카드이고 형제 순서다. `empty`는 카드가 없을 때 상자 크기다.
 */
export type NestGroup = { parent: number | null; card: NodeId | null; members: readonly NodeId[]; empty: Size };

/**
 * 저장소 하나의 자동 배치(목업 docs/mockups/arrange-family.html v4, 2026-10-21 사람 승인).
 * 자식 워크트리 묶음은 부모 카드의 묶음 상자 안, 부모 카드 다음 세대 줄에 선다. 부모 카드는 자식 전체(같은 묶음 카드와 안에 든 자식 묶음)의 가운데에 선다.
 * 세대는 저장소 전체에서 한 줄이다. 카드끼리 틈은 20, 상자가 낀 틈은 32다. 형제 나무는 앞 나무의 가장 깊은 줄보다 깊은 줄을 앞 나무 오른쪽 끝 뒤에 둔다(M11 규칙 5).
 * 맨 위 묶음 가운데 자식 묶음이 없는 묶음(빈 묶음 포함)은 나무 뒤에 선다(같은 목업 ⑫).
 * 맨 위 묶음의 자식 없는 뿌리는 제 줄만 차지한다(docs/mockups/arrange-root.html v2, `ownRow`).
 * `parentOf`는 카드의 부모 카드다. 돌려주는 값은 카드와 묶음 상자의 왼쪽 위다(가장 위·왼쪽이 0).
 */
export function arrangeNested(
  groups: readonly NestGroup[],
  parentOf: (id: NodeId) => NodeId | null,
  size: (id: NodeId) => Size,
  direction: ArrangeDirection,
  /** 묶음 상자의 안쪽 여백. `top`은 머리 줄이다(Graph.tsx `cover`와 같은 모양). */
  inset: { top: number; side: number },
): { cards: Map<NodeId, Point>; boxes: Box[] } {
  const vertical = direction === 'down' || direction === 'up';
  const reversed = direction === 'up' || direction === 'left';
  const across = (s: Size) => (vertical ? s.width : s.height);
  const along = (s: Size) => (vertical ? s.height : s.width);
  // 묶음 상자 여백. 가로지르는 축은 앞(lo)·뒤(hi), 뻗는 축은 뿌리 쪽(near)·먼 쪽(far)이다. 머리 줄은 늘 위다.
  const pad = {
    lo: vertical ? inset.side : inset.top,
    hi: inset.side,
    near: direction === 'down' ? inset.top : inset.side,
    far: direction === 'up' ? inset.top : inset.side,
  };
  const groupOf = new Map(groups.flatMap((g, i) => g.members.map((id): [NodeId, number] => [id, i])));
  const kidCards = (id: NodeId) => groups[groupOf.get(id)!].members.filter((k) => parentOf(k) === id);
  const kidGroups = (id: NodeId) => groups.flatMap((g, i) => (g.card === id ? [i] : []));
  const nestedIn = (i: number) => groups.flatMap((g, k) => (g.parent === i ? [k] : []));
  const roots = (i: number) => groups[i].members.filter((id) => {
    const up = parentOf(id);
    return up === null || groupOf.get(up) !== i;
  });
  // 카드의 세대 줄. 묶음의 뿌리 카드는 부모 카드 다음 줄이고, 맨 위 묶음의 뿌리는 0이다.
  const rows = new Map<NodeId, number>();
  const rowOf = (id: NodeId): number => {
    if (rows.has(id)) return rows.get(id)!;
    const i = groupOf.get(id)!;
    const up = parentOf(id);
    const card = up !== null && groupOf.get(up) === i ? up : groups[i].card;
    const r = card === null ? 0 : rowOf(card) + 1;
    rows.set(id, r);
    return r;
  };

  // 나무 한 덩이: 가로지르는 축의 시작 자리, 묶음 상자 양 끝과 줄 범위, 줄마다 양 끝과 그 끝이 카드인지 상자인지.
  type Edge = { at: number; box: boolean };
  type Shape = { at: Map<NodeId, number>; boxes: Map<number, { lo: number; hi: number; r0: number; r1: number }>; lo: Map<number, Edge>; hi: Map<number, Edge> };
  const shift = (s: Shape, by: number): Shape => ({
    at: new Map([...s.at].map(([k, x]) => [k, x + by])),
    boxes: new Map([...s.boxes].map(([k, b]) => [k, { ...b, lo: b.lo + by, hi: b.hi + by }])),
    lo: new Map([...s.lo].map(([k, e]) => [k, { ...e, at: e.at + by }])),
    hi: new Map([...s.hi].map(([k, e]) => [k, { ...e, at: e.at + by }])),
  });
  const merged = (a: Map<number, Edge>, b: Map<number, Edge>, outer: (x: number, y: number) => boolean) => {
    const out = new Map(a);
    for (const [k, e] of b) if (!out.has(k) || outer(e.at, out.get(k)!.at)) out.set(k, e);
    return out;
  };
  const join = (a: Shape, b: Shape): Shape => ({
    at: new Map([...a.at, ...b.at]),
    boxes: new Map([...a.boxes, ...b.boxes]),
    lo: merged(a.lo, b.lo, (x, y) => x < y),
    hi: merged(a.hi, b.hi, (x, y) => x > y),
  });
  // 뒤 나무의 깊은 줄(앞 나무의 가장 깊은 줄보다 깊은 줄)은 앞 나무들의 `claim` 가운데 가장 오른쪽 뒤에 선다. -Infinity만 있으면 막지 않는다.
  const rightOf = (s: Shape) => Math.max(...[...s.hi.values()].map((e) => e.at));
  const pack = (shapes: readonly Shape[], claim: (s: Shape) => number = rightOf): Shape | null => {
    let acc: Shape | null = null;
    let right = -Infinity;
    for (const s of shapes) {
      if (!acc) {
        acc = s;
        right = claim(s);
        continue;
      }
      const deepest = Math.max(...acc.hi.keys());
      let by = -Infinity;
      for (const [r, lo] of s.lo) {
        const edge = acc.hi.get(r) ?? (r > deepest && right > -Infinity ? { at: right, box: true } : undefined);
        if (edge) by = Math.max(by, edge.at + (edge.box || lo.box ? GROUP_GAP : CARD_LANE) - lo.at);
      }
      const moved = shift(s, by === -Infinity ? 0 : by);
      acc = join(acc, moved);
      right = Math.max(right, claim(moved));
    }
    return acc;
  };
  const layCard = (id: NodeId): Shape => {
    const r = rowOf(id);
    const kids = pack([...kidCards(id).map(layCard), ...kidGroups(id).map(layGroup)]);
    const own = across(size(id));
    // 부모는 다음 줄 자식 전체의 가운데에 선다.
    const x = kids ? (kids.lo.get(r + 1)!.at + kids.hi.get(r + 1)!.at) / 2 - own / 2 : 0;
    const me: Shape = { at: new Map([[id, x]]), boxes: new Map(), lo: new Map([[r, { at: x, box: false }]]), hi: new Map([[r, { at: x + own, box: false }]]) };
    return kids ? join(kids, me) : me;
  };
  // 맨 위 묶음의 자식 없는 뿌리는 제 줄만 차지한다. 뒤 나무의 깊은 줄은 그 아래로 들어온다(목업 docs/mockups/arrange-root.html v2, 2026-10-21 사람 승인).
  const ownRow = (s: Shape) => (s.hi.size === 1 ? -Infinity : rightOf(s));
  const layGroup = (i: number): Shape => {
    if (groups[i].members.length === 0) {
      const w = across(groups[i].empty);
      return { at: new Map(), boxes: new Map([[i, { lo: 0, hi: w, r0: 0, r1: 0 }]]), lo: new Map([[0, { at: 0, box: true }]]), hi: new Map([[0, { at: w, box: true }]]) };
    }
    const s = pack(roots(i).map(layCard), groups[i].card === null ? ownRow : rightOf)!;
    const rs = [...s.lo.keys()];
    const [r0, r1] = [Math.min(...rs), Math.max(...rs)];
    const lo = Math.min(...[...s.lo.values()].map((e) => e.at)) - pad.lo;
    const hi = Math.max(...[...s.hi.values()].map((e) => e.at)) + pad.hi;
    s.boxes.set(i, { lo, hi, r0, r1 });
    for (let r = r0; r <= r1; r++) {
      s.lo.set(r, { at: lo, box: true });
      s.hi.set(r, { at: hi, box: true });
    }
    return s;
  };
  const top = groups.flatMap((g, i) => (g.card === null ? [i] : []));
  const linked = (i: number) => groups[i].members.some((id) => kidGroups(id).length > 0);
  top.sort((a, b) => Number(!linked(a)) - Number(!linked(b)));
  const all = pack(top.map(layGroup));
  if (!all) return { cards: new Map(), boxes: [] };
  const min = Math.min(...[...all.boxes.values()].map((b) => b.lo));

  // 뻗는 축: 줄 크기는 그 줄에서 가장 큰 카드다. 줄 뒤 틈은 거기서 끝나는 상자들의 먼 쪽 여백, 상자 틈, 다음 줄 상자의 뿌리 쪽 여백이다.
  const cards = groups.flatMap((g) => g.members);
  const last = Math.max(0, ...cards.map(rowOf));
  const rowSize = Array.from({ length: last + 1 }, (_, r) => Math.max(0, ...cards.filter((id) => rowOf(id) === r).map((id) => along(size(id)))));
  const endRow = (i: number) => all.boxes.get(i)!.r1;
  const closing = (i: number): number => 1 + Math.max(0, ...nestedIn(i).filter((k) => endRow(k) === endRow(i)).map(closing));
  const close = (r: number) => Math.max(1, ...groups.flatMap((_, i) => (endRow(i) === r ? [closing(i)] : [])));
  const start: number[] = [];
  for (let r = 0; r <= last; r++) start[r] = r === 0 ? pad.near : start[r - 1] + rowSize[r - 1] + pad.far * close(r - 1) + GROUP_GAP + pad.near;
  // 묶음 상자의 뻗는 축 끝: 자기 카드와 안에 든 상자 가운데 가장 먼 곳에 먼 쪽 여백을 더한다.
  const end = (i: number): number => {
    if (groups[i].members.length === 0) return along(groups[i].empty);
    return Math.max(...groups[i].members.map((id) => start[rowOf(id)] + along(size(id))), ...nestedIn(i).map(end)) + pad.far;
  };
  const total = Math.max(...top.map(end));
  // 위로·왼쪽으로는 뻗는 축을 뒤집는다.
  const place = (a: number, length: number, c: number): Point => {
    const t = reversed ? total - a - length : a;
    return vertical ? { x: c, y: t } : { x: t, y: c };
  };
  return {
    cards: new Map(cards.map((id) => [id, place(start[rowOf(id)], along(size(id)), all.at.get(id)! - min)])),
    boxes: groups.map((g, i) => {
      const b = all.boxes.get(i)!;
      const t = g.members.length === 0 ? 0 : start[b.r0] - pad.near;
      const e = end(i);
      const at = place(t, e - t, b.lo - min);
      return { at, size: vertical ? { width: b.hi - b.lo, height: e - t } : { width: e - t, height: b.hi - b.lo } };
    }),
  };
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

/** `before`에 있던 상자 가운데 폭이나 높이가 작아졌거나 사라진 상자의 이름. 밀린 상자가 밀기 전 자리로 돌아갈 때다. */
export function shrank(before: ReadonlyMap<string, Size>, now: ReadonlyMap<string, Size>): string[] {
  return [...before].flatMap(([key, was]) => {
    const size = now.get(key);
    return !size || size.width < was.width || size.height < was.height ? [key] : [];
  });
}

/**
 * 사람 결정(2026-10-21) "권장안대로": 끌지 않고 커진 상자가 민 상자는 민 상자가 줄거나 사라지면 밀기 전 자리(`home`) 쪽으로 돌아간다.
 * `box`를 `home`까지 곧게 옮기되 다른 형제와 `gap` 안으로 들어가기 직전에 멈춘다. 지금 이미 `gap` 안에 있는 형제는 막지 않는다. 돌아갈 자리(왼쪽 위)를 준다.
 */
export function retreat(siblings: readonly Box[], box: Box, home: Point, gap: number): Point {
  const d = { x: home.x - box.at.x, y: home.y - box.at.y };
  let t = 1;
  for (const o of siblings) {
    const wide = { at: { x: o.at.x - gap, y: o.at.y - gap }, size: { width: o.size.width + 2 * gap, height: o.size.height + 2 * gap } };
    if (overlaps(box, wide)) continue;
    // 한 축에서 겹치는 t 구간(열린 구간). 움직이지 않는 축은 늘 겹치거나 늘 겹치지 않는다.
    const span = (p: number, len: number, dp: number, lo: number, hi: number): [number, number] => {
      if (dp === 0) return p < hi && p + len > lo ? [-Infinity, Infinity] : [Infinity, -Infinity];
      const [a, b] = [(lo - len - p) / dp, (hi - p) / dp];
      return a < b ? [a, b] : [b, a];
    };
    const [x0, x1] = span(box.at.x, box.size.width, d.x, wide.at.x, wide.at.x + wide.size.width);
    const [y0, y1] = span(box.at.y, box.size.height, d.y, wide.at.y, wide.at.y + wide.size.height);
    const [enter, leave] = [Math.max(x0, y0), Math.min(x1, y1)];
    if (enter < leave && enter < t && leave > 0) t = Math.max(0, enter);
  }
  return { x: box.at.x + t * d.x, y: box.at.y + t * d.y };
}

/** 저장한 자리와 밀기 전 자리. */
export type Kept = { at: Point; home: Point | null };

/** 끌지 않고 커진 상자가 `before`에 선 상자를 밀 때 저장할 밀기 전 자리. 또 밀려도 처음 자리를 지킨다. */
export function pushedHome(saved: Kept | undefined, before: Point): Point {
  return saved?.home ?? before;
}

/** 밀기와 되돌리기 밖에서 `at`에 저장하는 상자의 밀기 전 자리. 그 자리에 그대로 선 상자는 지난 값을 지키고, 옮긴 상자(사람이 끈 상자, 끈 상자가 민 상자, [정렬])는 잊는다. */
export function keptHome(saved: Kept | undefined, at: Point): Point | null {
  return saved && samePoint(saved.at, at) ? saved.home : null;
}

/** 밀기 전 자리로 돌아간 상자는 그 자리를 잊는다. */
export function samePoint(a: Point, b: Point): boolean {
  return Math.abs(a.x - b.x) < 0.5 && Math.abs(a.y - b.y) < 0.5;
}

/** `now`의 자리가 모두 `saved`에 있고 같은가. 정렬한 모양을 지키는 저장소는 다르면 `now`(새 자동 배치)를 저장한다. */
export function sameSpots(saved: ReadonlyMap<string, Point>, now: ReadonlyMap<string, Point>): boolean {
  return [...now].every(([key, p]) => {
    const s = saved.get(key);
    return s !== undefined && samePoint(s, p);
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

/** 부모 카드 끝에서 자식 카드까지 나무가 뻗는 쪽으로 잰 틈. 음수면 자식이 나무가 뻗는 쪽에 있지 않다. */
function aheadGap(parent: Box, child: Box, direction: ArrangeDirection): number {
  switch (direction) {
    case 'down':
      return child.at.y - (parent.at.y + parent.size.height);
    case 'up':
      return parent.at.y - (child.at.y + child.size.height);
    case 'right':
      return child.at.x - (parent.at.x + parent.size.width);
    case 'left':
      return parent.at.x - (child.at.x + child.size.width);
  }
}

/**
 * 나무 간선(계단 모양)이 꺾이는 자리. 부모 카드에서 `bend`만큼 간 곳이고, 부모와 자식 사이 틈에 대한 비율로 준다(React Flow `stepPosition`).
 * 자식이 나무가 뻗는 쪽에 없으면 null이다. 그 간선은 곡선이다. 틈이 `bend`의 두 배보다 좁으면 가운데에서 꺾는다.
 * 사람 승인(2026-10-21) "꺽은선 유지 하는걸로 승인", 목업 docs/mockups/arrange-mixed.html v5 규칙 6: 자식 묶음 상자는 부모 카드 아래에 있어 간선이 카드를 지나지 않는다(arrange-family.html v4).
 */
export function stepAt(parent: Box, child: Box, direction: ArrangeDirection, bend: number): number | null {
  const gap = aheadGap(parent, child, direction);
  if (gap < 0) return null;
  return gap >= 2 * bend ? bend / gap : 0.5;
}

/**
 * 부모와 자식 카드(화면 기준 상자)를 잇는 간선의 변. 자식이 부모보다 나무가 뻗는 쪽에 있으면 `TREE_SIDES[direction]`이다.
 * 아니면(다른 묶음의 자식) 두 카드 사이 빈 틈이 더 큰 축을 따라 서로 마주 보는 변이다.
 * 중심 거리로 축을 고르면 가로로 겹친 카드(틈이 좁다)에서 간선이 카드 옆 틈을 돌아 나가 꺾인다.
 */
export function edgeSides(parent: Box, child: Box, direction: ArrangeDirection): { parent: Side; child: Side } {
  if (aheadGap(parent, child, direction) >= 0) return TREE_SIDES[direction];
  const gapRight = child.at.x - (parent.at.x + parent.size.width);
  const gapLeft = parent.at.x - (child.at.x + child.size.width);
  const gapDown = child.at.y - (parent.at.y + parent.size.height);
  const gapUp = parent.at.y - (child.at.y + child.size.height);
  const horizontal = Math.max(gapRight, gapLeft) > Math.max(gapDown, gapUp);
  const toward: ArrangeDirection = horizontal ? (gapRight > gapLeft ? 'right' : 'left') : gapDown > gapUp ? 'down' : 'up';
  return TREE_SIDES[toward];
}
