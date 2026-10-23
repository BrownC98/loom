import { describe, expect, it } from 'vitest';
import { filePath, plainText } from './links';

describe('plainText', () => {
  it('keeps only labels', () => {
    expect(plainText([{ part: 'text', text: 'see ' }, { part: 'link', label: 'b.html', href: 'file:///x/b.html' }, { part: 'doc', label: 'R9' }, { part: 'text', text: '.' }])).toBe('see b.htmlR9.');
  });
});

describe('filePath', () => {
  const path = (uri: string) => filePath(new URL(uri));
  it('keeps the UNC host', () => {
    expect(path('file://wsl.localhost/Ubuntu-24.04/tmp/a%20b.html')).toBe('//wsl.localhost/Ubuntu-24.04/tmp/a b.html');
    expect(path('file:////wsl.localhost/Ubuntu-24.04/tmp/a%20b.html')).toBe('//wsl.localhost/Ubuntu-24.04/tmp/a b.html');
  });
  it('keeps POSIX and drive paths and folds extra slashes', () => {
    expect(path('file:///home/a%20b.html')).toBe('/home/a b.html');
    expect(path('file:///C:/Users/a.html')).toBe('/C:/Users/a.html');
    expect(path('file:////C:/Users/a.html')).toBe('/C:/Users/a.html');
    expect(path('file:///G:/%EB%82%B4%20%EB%93%9C%EB%9D%BC%EC%9D%B4%EB%B8%8C/a.html')).toBe(`/G:/${decodeURIComponent('%EB%82%B4%20%EB%93%9C%EB%9D%BC%EC%9D%B4%EB%B8%8C')}/a.html`);
  });
});
