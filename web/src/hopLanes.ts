import type { ArrangeDirection, Box } from './layout';

type Point = { x: number; y: number };

const FULL_TURN = 360;
const HUE_TRIES = 12;
const END_GAP = 40;
const LABEL_SPOTS = [0.5, 0.3, 0.7, 0.15, 0.85, 0.05, 0.95];
const LABEL_GAP = 2;
const BEND_GAP = 16;
const BOX_GAP = 8;

export type HopEnd = { id: number; box: Box; room: Box; bend: number | null };
export type LabelSize = { width: number; height: number };
export type HopPair = { key: string; source: HopEnd; target: HopEnd; label: { full: LabelSize; short: LabelSize } };
export type HopRoute = { points: Point[]; label: { at: Point; short: boolean } | null; turned: boolean };

export type HopEndName = 'source' | 'target';
export type HopPart = { from: HopEndName; to: HopEndName; back: boolean };

export function hopParts(counts: Readonly<Record<HopEndName, number>>, source: Point | null, target: Point | null, turned: boolean): HopPart[] {
  const sourceFirst = !source || !target || (turned ? source.y >= target.y : source.x <= target.x);
  const [first, second]: HopEndName[] = sourceFirst ? ['source', 'target'] : ['target', 'source'];
  const parts: HopPart[] = [
    { from: first, to: second, back: false },
    { from: second, to: first, back: true },
  ];
  return parts.filter((part) => counts[part.to] > 0);
}

export function pickHue(shown: readonly number[]): number {
  const gap = (hue: number) => Math.min(FULL_TURN, ...shown.map((s) => Math.min(Math.abs(hue - s), FULL_TURN - Math.abs(hue - s))));
  const tries = Array.from({ length: HUE_TRIES }, () => Math.floor(Math.random() * FULL_TURN));
  return tries.reduce((best, hue) => (gap(hue) > gap(best) ? hue : best));
}

export const hopColor = (hue: number) => `light-dark(oklch(55% 0.17 ${hue}), oklch(72% 0.15 ${hue}))`;

const toDown: Record<ArrangeDirection, (p: Point) => Point> = {
  down: (p) => p,
  up: (p) => ({ x: p.x, y: -p.y }),
  right: (p) => ({ x: p.y, y: p.x }),
  left: (p) => ({ x: p.y, y: -p.x }),
};

const fromDown: Record<ArrangeDirection, (p: Point) => Point> = {
  down: (p) => p,
  up: (p) => ({ x: p.x, y: -p.y }),
  right: (p) => ({ x: p.y, y: p.x }),
  left: (p) => ({ x: -p.y, y: p.x }),
};

type Rect = { l: number; t: number; r: number; b: number };

function rectDown(box: Box, direction: ArrangeDirection): Rect {
  const a = toDown[direction](box.at);
  const b = toDown[direction]({ x: box.at.x + box.size.width, y: box.at.y + box.size.height });
  return { l: Math.min(a.x, b.x), t: Math.min(a.y, b.y), r: Math.max(a.x, b.x), b: Math.max(a.y, b.y) };
}

export function siblingLanes(parent: Box, pairs: readonly HopPair[], direction: ArrangeDirection): Map<string, HopRoute> {
  const routes = new Map<string, HopRoute>();
  const up = rectDown(parent, direction);
  const rects = new Map<number, Rect>();
  const rooms = new Map<number, Rect>();
  for (const pair of pairs) {
    for (const end of [pair.source, pair.target]) {
      rects.set(end.id, rectDown(end.box, direction));
      rooms.set(end.id, rectDown(end.room, direction));
    }
  }
  const mid = (id: number) => (rects.get(id)!.l + rects.get(id)!.r) / 2;
  const room = Math.max(...pairs.map((p) => p.label.full.height)) + LABEL_GAP;
  const bus = (end: HopEnd) => up.b + (end.bend ?? (rects.get(end.id)!.t - up.b) / 2);
  const bendLine = (p: HopPair) => Math.max(bus(p.source), bus(p.target));
  const headTop = (p: HopPair) => Math.min(rects.get(p.source.id)!.t, rects.get(p.target.id)!.t) - LABEL_GAP;
  const wanted = (p: HopPair) => bendLine(p) + Math.max(BEND_GAP, room / 2);
  const busLine = (p: HopPair) => (headTop(p) >= wanted(p) ? wanted(p) : (bendLine(p) + headTop(p)) / 2);
  const labelled = (p: HopPair) => headTop(p) - busLine(p) >= room / 2;
  const band = (p: HopPair) => ({ lo: busLine(p), hi: labelled(p) ? headTop(p) - room / 2 : headTop(p) });
  const below = pairs.filter((p) => headTop(p) >= busLine(p));
  if (below.length === 0) return routes;
  const span = (p: HopPair) => Math.abs(mid(p.source.id) - mid(p.target.id));
  const left = (p: HopPair) => Math.min(mid(p.source.id), mid(p.target.id));
  const order = [...below].sort((a, b) => span(a) - span(b) || left(a) - left(b) || (a.key < b.key ? -1 : 1));

  type End = { pair: number; id: number; toward: number };
  const ends = new Map<number, End[]>();
  order.forEach((p, i) => {
    for (const [end, other] of [[p.source, p.target], [p.target, p.source]] as const) {
      const list = ends.get(end.id) ?? [];
      list.push({ pair: i, id: end.id, toward: Math.sign(mid(other.id) - mid(end.id)) });
      ends.set(end.id, list);
    }
  });
  const at = new Map<string, number>();
  for (const [id, list] of ends) {
    list.sort((a, b) => a.toward - b.toward || (a.toward < 0 ? a.pair - b.pair : b.pair - a.pair));
    const r = rects.get(id)!;
    const gap = Math.min(END_GAP, (r.r - r.l) / (list.length + 1));
    list.forEach((e, k) => at.set(`${e.pair}:${id}`, mid(id) + (k - (list.length - 1) / 2) * gap));
  }
  const reach = (i: number) => {
    const xa = at.get(`${i}:${order[i].source.id}`)!;
    const xb = at.get(`${i}:${order[i].target.id}`)!;
    return { l: Math.min(xa, xb), r: Math.max(xa, xb) };
  };
  const boxes = [...rooms].filter(([id, q]) => q.t < rects.get(id)!.t).map(([id, q]) => ({ q, card: rects.get(id)!.t - LABEL_GAP - room }))
    .sort((a, b) => b.q.t - a.q.t);
  const lanes: number[] = [];
  order.forEach((p, i) => {
    const x = reach(i);
    const nested = lanes.filter((_, j) => reach(j).l < x.r && x.l < reach(j).r);
    let y = Math.min(band(p).hi, ...nested.map((n) => n - room));
    for (const { q, card } of boxes) if (q.l < x.r && x.l < q.r && y > q.t - BOX_GAP && y < card) y = q.t - BOX_GAP;
    lanes.push(Math.max(band(p).lo, y));
  });

  const apart = (r: Rect, q: Rect, gap: number) => r.r + gap <= q.l || q.r + gap <= r.l || r.b + gap <= q.t || q.b + gap <= r.t;
  const heads = pairs.flatMap((p) => [p.source, p.target]).filter((e) => e.room.at.y < e.box.at.y).map((e) => rectDown({ at: e.room.at, size: { width: e.room.size.width, height: e.box.at.y - e.room.at.y - LABEL_GAP - room } }, direction));
  const walls = [up, ...rects.values(), ...heads];
  const placed: Rect[] = [];
  const stems = order.flatMap((p, i) =>
    [p.source.id, p.target.id].map((id) => {
      const x = at.get(`${i}:${id}`)!;
      return { pair: i, rect: { l: x - 1, t: lanes[i], r: x + 1, b: rects.get(id)!.t } };
    }),
  );
  order.forEach((p, i) => {
    const clear = (r: Rect) => walls.every((q) => apart(r, q, 0)) && placed.every((q) => apart(r, q, LABEL_GAP)) && stems.every((s) => s.pair === i || apart(r, s.rect, 0));
    const y = lanes[i];
    const xa = at.get(`${i}:${p.source.id}`)!;
    const xb = at.get(`${i}:${p.target.id}`)!;
    const fit = (size: LabelSize) => {
      const box = (t: number): Rect => {
        const x = xa + (xb - xa) * t;
        return { l: x - size.width / 2, t: y - size.height / 2, r: x + size.width / 2, b: y + size.height / 2 };
      };
      const t = labelled(p) ? LABEL_SPOTS.find((t) => clear(box(t))) : undefined;
      return t === undefined ? null : box(t);
    };
    const full = fit(p.label.full);
    const spot = full ?? fit(p.label.short);
    if (spot) placed.push(spot);
    const back = fromDown[direction];
    routes.set(p.key, {
      points: [{ x: xa, y: rects.get(p.source.id)!.t }, { x: xa, y }, { x: xb, y }, { x: xb, y: rects.get(p.target.id)!.t }].map(back),
      label: spot && { at: back({ x: (spot.l + spot.r) / 2, y }), short: !full },
      turned: direction === 'right' || direction === 'left',
    });
  });
  return routes;
}
