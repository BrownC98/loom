import { readdirSync } from 'node:fs';
import { join } from 'node:path';

function* files(dir) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) yield* files(path);
    else if (/\.tsx?$/.test(entry.name)) yield path;
  }
}

const seen = new Map();
let found = 0;
for (const path of files('src')) {
  const key = path.replace(/(\.test)?\.tsx?$/, '$1').toLowerCase();
  const other = seen.get(key);
  if (other) {
    console.error(`${other} and ${path} differ only in letter case. Windows sees one module.`);
    found++;
  } else seen.set(key, path);
}
process.exit(found ? 1 : 0);
