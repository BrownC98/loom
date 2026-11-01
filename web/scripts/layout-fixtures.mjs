import { deepStrictEqual } from 'node:assert/strict';
import { readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { arrangeNested, settle } from '../src/layout.ts';

const dir = fileURLToPath(new URL('../../fixtures/layout/', import.meta.url));
const write = process.argv.includes('--write');
const point = (p) => ({ x: p.x, y: p.y });

const run = {
  arrangeNested: (i) => {
    const out = arrangeNested(
      i.groups,
      (id) => i.parents[id] ?? null,
      (id) => i.sizes[id],
      i.direction,
      i.inset,
      (kid) => i.rank['card' in kid ? `card:${kid.card}` : `group:${kid.group}`] ?? Infinity,
    );
    return { cards: Object.fromEntries([...out.cards].map(([id, p]) => [id, point(p)])), boxes: out.boxes };
  },
  settle: (i) => Object.fromEntries([...settle(i.items, (key) => i.placed[key], i.gap)].map(([key, p]) => [key, point(p)])),
};

let failed = false;
for (const name of readdirSync(dir).filter((f) => f.endsWith('.json')).sort()) {
  const file = join(dir, name);
  const fixture = JSON.parse(readFileSync(file, 'utf8'));
  const actual = JSON.parse(JSON.stringify(run[fixture.kind](fixture.input)));
  if (write) {
    writeFileSync(file, `${JSON.stringify({ ...fixture, output: actual }, null, 2)}\n`);
    continue;
  }
  try {
    deepStrictEqual(actual, fixture.output);
  } catch (err) {
    failed = true;
    console.error(`${name}: ${err.message}`);
  }
}
if (failed) process.exit(1);
