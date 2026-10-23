import type { DocGroup } from './gen/DocGroup';
import type { DocRow } from './gen/DocRow';

/** 고른 종류이고 ID나 제목에 찾는 글자가 있는 문서. 순서는 데몬이 정한 그대로다(최근에 붙인 것 위). */
export function shownDocs(docs: readonly DocRow[], hidden: ReadonlySet<DocGroup>, query: string): DocRow[] {
  const needle = query.trim().toLowerCase();
  return docs.filter((d) => !hidden.has(d.kind) && (needle === '' || d.id.toLowerCase().includes(needle) || d.title.toLowerCase().includes(needle)));
}
