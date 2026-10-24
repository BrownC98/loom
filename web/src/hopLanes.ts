import type { ArrangeDirection, Box } from './layout';

type Point = { x: number; y: number };

export const HOP_LANES = [2, 4, 5, 1, 3] as const;
const LANE_MARGIN = 10;
const LANE_STEP = 10;
const END_GAP = 40;

export type HopEnd = { id: number; box: Box };
export type HopPair = { key: string; source: HopEnd; target: HopEnd };
export type HopRoute = { points: Point[]; label: Point };

export function pickColor(shown: Iterable<number>): number {
  const uses = HOP_LANES.map(() => 0);
  for (const color of shown) uses[color % HOP_LANES.length]++;
  return uses.indexOf(Math.min(...uses));
}

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
  for (const pair of pairs) for (const end of [pair.source, pair.target]) rects.set(end.id, rectDown(end.box, direction));
  const mid = (id: number) => (rects.get(id)!.l + rects.get(id)!.r) / 2;
  const below = pairs.filter((p) => rects.get(p.source.id)!.t - up.b > 2 * LANE_MARGIN && rects.get(p.target.id)!.t - up.b > 2 * LANE_MARGIN);
  if (below.length === 0) return routes;
  const span = (p: HopPair) => Math.abs(mid(p.source.id) - mid(p.target.id));
  const left = (p: HopPair) => Math.min(mid(p.source.id), mid(p.target.id));
  const order = [...below].sort((a, b) => span(a) - span(b) || left(a) - left(b) || (a.key < b.key ? -1 : 1));
  const lo = up.b + LANE_MARGIN;
  const hi = Math.min(...order.flatMap((p) => [rects.get(p.source.id)!.t, rects.get(p.target.id)!.t])) - LANE_MARGIN;
  const step = order.length > 1 ? Math.min(LANE_STEP, (hi - lo) / (order.length - 1)) : 0;
  const lane = (i: number) => lo + (order.length - 1 - i) * step;

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

  order.forEach((p, i) => {
    const y = lane(i);
    const xa = at.get(`${i}:${p.source.id}`)!;
    const xb = at.get(`${i}:${p.target.id}`)!;
    const back = fromDown[direction];
    routes.set(p.key, {
      points: [{ x: xa, y: rects.get(p.source.id)!.t }, { x: xa, y }, { x: xb, y }, { x: xb, y: rects.get(p.target.id)!.t }].map(back),
      label: back({ x: (xa + xb) / 2, y }),
    });
  });
  return routes;
}
