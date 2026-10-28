import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

const ROOT = 'src';
const REDUCED = /@media[^{]*prefers-reduced-motion[^{]*\{/g;

let found = 0;
for (const name of readdirSync(ROOT).filter((n) => n.endsWith('.css'))) {
  const path = join(ROOT, name);
  const css = readFileSync(path, 'utf8');
  for (const match of css.matchAll(REDUCED)) {
    let depth = 1;
    let end = match.index + match[0].length;
    while (depth > 0 && end < css.length) depth += { '{': 1, '}': -1 }[css[end++]] ?? 0;
    if (css.slice(match.index, end).includes('react-flow__edge')) {
      console.error(`${path}:${css.slice(0, match.index).split('\n').length}: ${css.slice(match.index, end)}`);
      found++;
    }
  }
}
if (found > 0) {
  console.error('메시지 간선의 점선 흐름은 메시지 방향을 보이는 정보다. 움직임 줄이기 설정에서도 끄지 않는다(Windows 「애니메이션 효과」 끔이 이 설정을 켠다).');
  process.exit(1);
}
