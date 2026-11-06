export type PathPoint = { x: number; y: number };

const CORNER = 5;

export function roundedPath(all: readonly PathPoint[]): string {
  const points = all.filter((p, i) => i === 0 || p.x !== all[i - 1].x || p.y !== all[i - 1].y);
  const toward = (from: number, to: number) => Math.sign(to - from) * Math.min(CORNER, Math.abs(to - from) / 2);
  const [first, ...rest] = points;
  let d = `M ${first.x} ${first.y}`;
  rest.forEach((p, i) => {
    const prev = points[i];
    const next = rest[i + 1];
    if (!next) {
      d += ` L ${p.x} ${p.y}`;
      return;
    }
    d += ` L ${p.x - toward(prev.x, p.x)} ${p.y - toward(prev.y, p.y)} Q ${p.x} ${p.y} ${p.x + toward(p.x, next.x)} ${p.y + toward(p.y, next.y)}`;
  });
  return d;
}
