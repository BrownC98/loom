import { describe, expect, it } from 'vitest';
import cases from './links.cases.json';
import { DOC_LINK, filePath, linkParts, plainText } from './links';

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
  it('keeps only labels in plain text', () => {
    expect(plainText('see [b.html](file:///x/b.html).')).toBe('see b.html.');
  });
  it('draws exactly the links the daemon accepts (shared cases)', () => {
    for (const { text, links } of cases.filter((c) => !c.refused)) {
      const hrefs = linkParts(text).flatMap((part) => ('href' in part ? [part.href] : []));
      expect(hrefs, text).toEqual(links);
    }
  });
});

describe('filePath', () => {
  const path = (uri: string) => filePath(new URL(uri));
  it('keeps the UNC host', () => {
    expect(path('file://wsl.localhost/Ubuntu-24.04/tmp/a%20b.html')).toBe('//wsl.localhost/Ubuntu-24.04/tmp/a b.html');
  });
  it('keeps POSIX and drive paths and folds extra slashes', () => {
    expect(path('file:///home/a%20b.html')).toBe('/home/a b.html');
    expect(path('file:///C:/Users/a.html')).toBe('/C:/Users/a.html');
    expect(path('file:////C:/Users/a.html')).toBe('/C:/Users/a.html');
    expect(path('file:///G:/%EB%82%B4%20%EB%93%9C%EB%9D%BC%EC%9D%B4%EB%B8%8C/a.html')).toBe('/G:/내 드라이브/a.html');
  });
});
