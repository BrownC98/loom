import type { ArrangeDirection } from './gen/ArrangeDirection';
import type { NodeId } from './gen/NodeId';

export type { ArrangeDirection };
export const ARRANGE_DIRECTIONS: readonly ArrangeDirection[] = ['down', 'right', 'up', 'left'];

export type Size = { width: number; height: number };
type Point = { x: number; y: number };
const CARD_LANE = 20;

export const GROUP_GAP = 32;

export type Box = { at: Point; size: Size };

export type NestGroup = { parent: number | null; card: NodeId | null; members: readonly NodeId[]; empty: Size };
export type Kid = { card: NodeId } | { group: number };

export function arrangeNested(
  groups: readonly NestGroup[],
  parentOf: (id: NodeId) => NodeId | null,
  size: (id: NodeId) => Size,
  direction: ArrangeDirection,
  inset: { top: number; side: number },
  rank: (kid: Kid) => number,
): { cards: Map<NodeId, Point>; boxes: Box[] } {
  const vertical = direction === 'down' || direction === 'up';
  const reversed = direction === 'up' || direction === 'left';
  const across = (s: Size) => (vertical ? s.width : s.height);
  const along = (s: Size) => (vertical ? s.height : s.width);
  const pad = {
    lo: vertical ? inset.side : inset.top,
    hi: inset.side,
    near: direction === 'down' ? inset.top : inset.side,
    far: direction === 'up' ? inset.top : inset.side,
  };
  const ahead = (a: number, b: number) => (a === b ? 0 : a < b ? -1 : 1);
  const byRank = (kids: Kid[]) => kids.sort((a, b) => ahead(rank(a), rank(b)));
  const groupOf = new Map(groups.flatMap((g, i) => g.members.map((id): [NodeId, number] => [id, i])));
  const kidCards = (id: NodeId) => groups[groupOf.get(id)!].members.filter((k) => parentOf(k) === id);
  const kidGroups = (id: NodeId) => groups.flatMap((g, i) => (g.card === id ? [i] : []));
  const nestedIn = (i: number) => groups.flatMap((g, k) => (g.parent === i ? [k] : []));
  const roots = (i: number) => groups[i].members.filter((id) => {
    const up = parentOf(id);
    return up === null || groupOf.get(up) !== i;
  });
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
    const kids = pack(byRank([...kidCards(id).map((card): Kid => ({ card })), ...kidGroups(id).map((group): Kid => ({ group }))]).map((k) => ('card' in k ? layCard(k.card) : layGroup(k.group))));
    const own = across(size(id));
    const x = kids ? (kids.lo.get(r + 1)!.at + kids.hi.get(r + 1)!.at) / 2 - own / 2 : 0;
    const me: Shape = { at: new Map([[id, x]]), boxes: new Map(), lo: new Map([[r, { at: x, box: false }]]), hi: new Map([[r, { at: x + own, box: false }]]) };
    return kids ? join(kids, me) : me;
  };
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
  top.sort((a, b) => ahead(rank({ group: a }), rank({ group: b })) || Number(!linked(a)) - Number(!linked(b)));
  const all = pack(top.map(layGroup));
  if (!all) return { cards: new Map(), boxes: [] };
  const min = Math.min(...[...all.boxes.values()].map((b) => b.lo));

  const cards = groups.flatMap((g) => g.members);
  const last = Math.max(0, ...cards.map(rowOf));
  const rowSize = Array.from({ length: last + 1 }, (_, r) => Math.max(0, ...cards.filter((id) => rowOf(id) === r).map((id) => along(size(id)))));
  const endRow = (i: number) => all.boxes.get(i)!.r1;
  const closing = (i: number): number => 1 + Math.max(0, ...nestedIn(i).filter((k) => endRow(k) === endRow(i)).map(closing));
  const close = (r: number) => Math.max(1, ...groups.flatMap((_, i) => (endRow(i) === r ? [closing(i)] : [])));
  const start: number[] = [];
  for (let r = 0; r <= last; r++) start[r] = r === 0 ? pad.near : start[r - 1] + rowSize[r - 1] + pad.far * close(r - 1) + GROUP_GAP + pad.near;
  const end = (i: number): number => {
    if (groups[i].members.length === 0) return along(groups[i].empty);
    return Math.max(...groups[i].members.map((id) => start[rowOf(id)] + along(size(id))), ...nestedIn(i).map(end)) + pad.far;
  };
  const total = Math.max(...top.map(end));
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

export type Sibling = Box & { key: string };

export const FREE_GAP = 32;

export function overlaps(a: Box, b: Box): boolean {
  return a.at.x < b.at.x + b.size.width && b.at.x < a.at.x + a.size.width && a.at.y < b.at.y + b.size.height && b.at.y < a.at.y + a.size.height;
}

const PUSH_SIDES = ['right', 'down', 'left', 'up'] as const;
type PushSide = (typeof PUSH_SIDES)[number];

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

export function push(siblings: readonly Sibling[], mover: Box, targets: readonly string[], gap: number): Map<string, Point> {
  return pushBy(siblings, mover, targets, gap, (box) => PUSH_SIDES.reduce((best, s) => (clearance(box, mover, s, gap) < clearance(box, mover, best, gap) ? s : best)));
}

function away(box: Box, mover: Box): PushSide {
  return (box.at.x - mover.at.x) / mover.size.width >= (box.at.y - mover.at.y) / mover.size.height ? 'right' : 'down';
}

function pushBy(siblings: readonly Sibling[], mover: Box, targets: readonly string[], gap: number, sideOf: (box: Box) => PushSide): Map<string, Point> {
  const now = new Map(siblings.map((s) => [s.key, s as Box]));
  const out = new Map<string, Point>();
  for (const key of targets) {
    const box = now.get(key);
    if (!box || !overlaps(box, mover)) continue;
    const side = sideOf(box);
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

export type CenterSide = { x: number; y: number };

const center = (b: Box) => ({ x: b.at.x + b.size.width / 2, y: b.at.y + b.size.height / 2 });

export function centerSide(dragged: Box, box: Box): CenterSide {
  const [d, o] = [center(dragged), center(box)];
  const apartX = dragged.at.x + dragged.size.width <= box.at.x || box.at.x + box.size.width <= dragged.at.x;
  const apartY = dragged.at.y + dragged.size.height <= box.at.y || box.at.y + box.size.height <= dragged.at.y;
  return { x: apartX ? Math.sign(d.x - o.x) : 0, y: apartY ? Math.sign(d.y - o.y) : 0 };
}

export function crossed(before: CenterSide, dragged: Box, box: Box): boolean {
  const o = center(box);
  const lead = { x: before.x < 0 ? dragged.at.x + dragged.size.width : dragged.at.x, y: before.y < 0 ? dragged.at.y + dragged.size.height : dragged.at.y };
  return (before.x !== 0 && Math.sign(lead.x - o.x) !== before.x) || (before.y !== 0 && Math.sign(lead.y - o.y) !== before.y);
}

export type Swap = { slot: Box; sides: ReadonlyMap<string, CenterSide>; at: ReadonlyMap<string, Point> };

export function swap(state: Swap, others: readonly Sibling[], dragged: Box): Swap {
  let slot = state.slot;
  const sides = new Map(state.sides);
  const at = new Map(state.at);
  for (const s of others) {
    const o = { ...s, at: at.get(s.key) ?? s.at };
    if (!overlaps(dragged, o)) {
      sides.set(o.key, centerSide(dragged, o));
      continue;
    }
    const before = sides.get(o.key) ?? centerSide(slot, o);
    sides.set(o.key, before);
    if (!crossed(before, dragged, o)) continue;
    const edge = (side: number, now: number, from: number, len: number, own: number) => (side === 0 ? now : side > 0 ? from + len - own : from);
    at.set(o.key, { x: edge(before.x, o.at.x, slot.at.x, slot.size.width, o.size.width), y: edge(before.y, o.at.y, slot.at.y, slot.size.height, o.size.height) });
    slot = o;
    sides.set(o.key, { x: -before.x, y: -before.y });
  }
  return { slot, sides, at };
}

export function freeSpot(box: Box, others: readonly Box[], gap: number): Point {
  let at = box.at;
  for (let hit = others.find((o) => overlaps({ at, size: box.size }, o)); hit; hit = others.find((o) => overlaps({ at, size: box.size }, o))) {
    at = { x: hit.at.x + hit.size.width + gap, y: at.y };
  }
  return at;
}

export function settle(items: readonly { key: string; size: Size; lead: Point; auto: Point }[], placed: (key: string) => Point | undefined, gap: number): Map<string, Point> {
  const at = new Map<string, Point>();
  const taken: Box[] = [];
  for (const item of items) {
    const saved = placed(item.key);
    if (saved) at.set(item.key, { x: saved.x + item.lead.x, y: saved.y + item.lead.y });
  }
  for (const [key, p] of at) taken.push({ at: p, size: items.find((i) => i.key === key)!.size });
  for (const item of items) {
    if (at.has(item.key)) continue;
    const p = freeSpot({ at: item.auto, size: item.size }, taken, gap);
    at.set(item.key, p);
    taken.push({ at: p, size: item.size });
  }
  return at;
}

export function unpile(siblings: readonly Sibling[], gap: number): Map<string, Point> {
  const now = new Map(siblings.map((s) => [s.key, s.at]));
  siblings.forEach((s, i) => {
    const boxes = siblings.map((b) => ({ ...b, at: now.get(b.key)! }));
    const mover = boxes[i];
    const later = boxes.slice(i + 1).filter((o) => overlaps(o, mover)).map((o) => o.key);
    for (const [k, p] of pushBy(boxes.filter((b) => b.key !== s.key), mover, later, gap, (box) => away(box, mover))) now.set(k, p);
  });
  return now;
}

export function samePoint(a: Point, b: Point): boolean {
  return Math.abs(a.x - b.x) < 0.5 && Math.abs(a.y - b.y) < 0.5;
}

export type Side = 'top' | 'right' | 'bottom' | 'left';
export const SIDES: readonly Side[] = ['top', 'right', 'bottom', 'left'];
export const TREE_SIDES: Record<ArrangeDirection, { parent: Side; child: Side }> = {
  down: { parent: 'bottom', child: 'top' },
  up: { parent: 'top', child: 'bottom' },
  right: { parent: 'right', child: 'left' },
  left: { parent: 'left', child: 'right' },
};

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

export function stepAt(parent: Box, child: Box, direction: ArrangeDirection, bend: number): number | null {
  const gap = aheadGap(parent, child, direction);
  if (gap < 0) return null;
  return gap >= 2 * bend ? bend / gap : 0.5;
}

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

export type EdgeEnd = { box: Box; side: Side };

function sidePoint({ box, side }: EdgeEnd): Point {
  const x = { left: box.at.x, right: box.at.x + box.size.width, top: box.at.x + box.size.width / 2, bottom: box.at.x + box.size.width / 2 }[side];
  const y = { top: box.at.y, bottom: box.at.y + box.size.height, left: box.at.y + box.size.height / 2, right: box.at.y + box.size.height / 2 }[side];
  return { x, y };
}

export function labelHeading(from: EdgeEnd, to: EdgeEnd, step: number | null): ArrangeDirection {
  const a = sidePoint(from);
  const b = sidePoint(to);
  const dx = Math.round(b.x - a.x);
  const dy = Math.round(b.y - a.y);
  const vertical = from.side === 'top' || from.side === 'bottom';
  const lean = step !== null ? (vertical ? { x: dx, y: 0 } : { x: 0, y: dy }) : vertical ? { x: 2 * dx, y: dy } : { x: dx, y: 2 * dy };
  const m = lean.x === 0 && lean.y === 0 ? { x: dx, y: dy } : lean;
  if (Math.abs(m.x) > Math.abs(m.y)) return m.x > 0 ? 'right' : 'left';
  return m.y > 0 ? 'down' : 'up';
}

export type Camera = { x: number; y: number; zoom: number };

export function followCard(old: Point, now: Point, camera: Camera): Camera {
  return { x: camera.x - (now.x - old.x) * camera.zoom, y: camera.y - (now.y - old.y) * camera.zoom, zoom: camera.zoom };
}
