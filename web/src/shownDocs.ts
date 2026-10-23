import type { DocGroup } from './gen/DocGroup';
import type { DocRow } from './gen/DocRow';

export function shownDocs(docs: readonly DocRow[], hidden: ReadonlySet<DocGroup>, query: string): DocRow[] {
  const needle = query.trim().toLowerCase();
  return docs.filter((d) => !hidden.has(d.kind) && (needle === '' || d.id.toLowerCase().includes(needle) || d.title.toLowerCase().includes(needle)));
}
