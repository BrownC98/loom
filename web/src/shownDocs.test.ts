import { describe, expect, it } from 'vitest';
import type { DocRow } from './gen/DocRow';
import { shownDocs } from './shownDocs';

const doc = (id: string, title: string): DocRow => ({ id, title, kind: id.startsWith('M') ? 'mockup' : 'report', folder: 'f', attachedAt: null, source: true, snapshot: true });

describe('shownDocs', () => {
  const docs = [doc('M42', 'Reopen docs'), doc('R17', 'Daemon result'), doc('M4', 'Phone')];

  it('keeps the daemon order and hides unchecked kinds', () => {
    expect(shownDocs(docs, new Set(), '').map((d) => d.id)).toEqual(['M42', 'R17', 'M4']);
    expect(shownDocs(docs, new Set(['mockup']), '').map((d) => d.id)).toEqual(['R17']);
  });

  it('matches the ID or the title, ignoring case', () => {
    expect(shownDocs(docs, new Set(), ' m4 ').map((d) => d.id)).toEqual(['M42', 'M4']);
    expect(shownDocs(docs, new Set(), 'daemon').map((d) => d.id)).toEqual(['R17']);
    expect(shownDocs(docs, new Set(['report']), 'daemon')).toEqual([]);
  });
});
