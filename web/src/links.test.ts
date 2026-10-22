import { describe, expect, it } from 'vitest';
import { linkParts, plainText } from './links';

describe('linkParts', () => {
  it('splits markdown links out of the text', () => {
    expect(linkParts('see [b.html](file:///home/a/%ED%98%91/b.html) now')).toEqual([
      { text: 'see ' },
      { label: 'b.html', href: 'file:///home/a/%ED%98%91/b.html' },
      { text: ' now' },
    ]);
  });
  it('keeps text without links as one part', () => {
    expect(linkParts('no link [x] (y)')).toEqual([{ text: 'no link [x] (y)' }]);
  });
  it('ignores other schemes', () => {
    expect(linkParts('[x](javascript:alert(1))')).toEqual([{ text: '[x](javascript:alert(1))' }]);
  });
  it('keeps only labels in plain text', () => {
    expect(plainText('see [b.html](file:///x/b.html).')).toBe('see b.html.');
  });
});
