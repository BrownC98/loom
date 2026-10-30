function sameShape(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (Array.isArray(a)) return Array.isArray(b) && a.length === b.length && a.every((v, i) => sameShape(v, b[i]));
  if (!isPlain(a) || !isPlain(b)) return false;
  const keys = Object.keys(a);
  return keys.length === Object.keys(b).length && keys.every((k) => k in b && sameShape(a[k], b[k]));
}

function isPlain(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && Object.getPrototypeOf(v) === Object.prototype;
}

export function keepSame<T extends { id: string | number }>(old: T[], next: T[]): T[] {
  const before = new Map(old.map((item) => [item.id, item]));
  const kept = next.map((item) => {
    const prev = before.get(item.id);
    return prev && sameShape(prev, item) ? prev : item;
  });
  return kept.length === old.length && kept.every((item, i) => item === old[i]) ? old : kept;
}
