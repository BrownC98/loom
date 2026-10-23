import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import ts from 'typescript';

const ROOT = 'src';
const SKIP = new Set([join(ROOT, 'locales'), join(ROOT, 'gen')]);
const HANGUL = /[ㄱ-ㆎ가-힣]/;

function* files(dir) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      if (!SKIP.has(path)) yield* files(path);
    } else if (/\.tsx?$/.test(entry.name)) yield path;
  }
}

let found = 0;
for (const path of files(ROOT)) {
  const source = ts.createSourceFile(path, readFileSync(path, 'utf8'), ts.ScriptTarget.Latest, true);
  const visit = (node) => {
    const text = ts.isStringLiteralLike(node) || ts.isTemplateLiteralToken(node) || ts.isJsxText(node) ? node.text : null;
    if (text !== null && HANGUL.test(text)) {
      const { line } = source.getLineAndCharacterOfPosition(node.getStart());
      console.error(`${path}:${line + 1}: ${text.trim()}`);
      found++;
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
}
if (found > 0) {
  console.error(`한글 문구 ${found}개: src/locales/에 키를 더하고 t()로 바꾼다.`);
  process.exit(1);
}
