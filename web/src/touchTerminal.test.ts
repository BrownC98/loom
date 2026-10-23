import { describe, expect, it, vi } from 'vitest';
import { hapticTick, wordBounds } from './touchTerminal';

const SEP = " ()[]{}',\"`";
const cells = (text: string) => Array.from(text);

describe('wordBounds', () => {
  it('picks the whole word under the cell', () => {
    expect(wordBounds(cells('git commit -m'), 5, SEP)).toEqual({ start: 4, end: 9 });
  });
  it('stops at separators', () => {
    expect(wordBounds(cells('f(abc)'), 3, SEP)).toEqual({ start: 2, end: 4 });
  });
  it('returns one cell on a space', () => {
    expect(wordBounds(cells('a  b'), 1, SEP)).toEqual({ start: 1, end: 1 });
  });
  it('keeps the second half of a wide char in the word', () => {
    expect(wordBounds(['中', '', '文', '', ' ', 'x'], 1, SEP)).toEqual({ start: 0, end: 3 });
  });
});

describe('hapticTick', () => {
  it('uses the app bridge when it exists, else the browser vibration', () => {
    const calls: string[] = [];
    vi.stubGlobal('window', { loomChrome: { haptic: () => calls.push('app') } });
    vi.stubGlobal('navigator', { vibrate: () => calls.push('web') });
    hapticTick();
    vi.stubGlobal('window', {});
    hapticTick();
    vi.unstubAllGlobals();
    expect(calls).toEqual(['app', 'web']);
  });
});
