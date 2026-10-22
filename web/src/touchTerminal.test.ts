import { describe, expect, it } from 'vitest';
import { wordBounds } from './touchTerminal';

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
