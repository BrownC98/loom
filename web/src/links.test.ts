import { describe, expect, it } from 'vitest';
import { DOC_LINK, linkParts, plainText } from './links';

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
  it('splits the attached doc link', () => {
    expect(linkParts(`open [R9 tool](${DOC_LINK}), then pick`)).toEqual([
      { text: 'open ' },
      { label: 'R9 tool', href: DOC_LINK },
      { text: ', then pick' },
    ]);
  });
  it('links bare urls without trailing punctuation', () => {
    const url = 'https://docs.google.com/spreadsheets/d/17N/edit';
    expect(linkParts(`done: ${url}.`)).toEqual([{ text: 'done: ' }, { label: url, href: url }, { text: '.' }]);
    expect(linkParts(`(${url})\u300d`)).toEqual([{ text: '(' }, { label: url, href: url }, { text: ')\u300d' }]);
  });
  it('does not link a url twice inside a markdown link', () => {
    expect(linkParts('[sheet](https://a.io/x) https://b.io/y')).toEqual([
      { label: 'sheet', href: 'https://a.io/x' },
      { text: ' ' },
      { label: 'https://b.io/y', href: 'https://b.io/y' },
    ]);
  });
  it('leaves non-http bare urls as text', () => {
    expect(linkParts('ftp://a.io file:///x')).toEqual([{ text: 'ftp://a.io file:///x' }]);
  });
  it('keeps only labels in plain text', () => {
    expect(plainText('see [b.html](file:///x/b.html).')).toBe('see b.html.');
  });
});
