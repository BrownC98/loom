// 모듈 이름 겹침 검사: 확장자를 뺀 경로가 대소문자만 다른 파일 둘이 있으면 실패한다.
// Windows는 파일 이름의 대소문자를 가리지 않아 `./Find`와 `./find`가 같은 파일이 된다. Linux 검사는 통과하고 Windows 빌드만 깨졌다(2026-10-21 release 39534245021).
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
