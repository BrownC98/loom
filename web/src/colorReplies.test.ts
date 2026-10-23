import { describe, expect, it } from 'vitest';
import { withoutColorReplies } from './colorReplies';

describe('withoutColorReplies', () => {
  it('drops OSC 10, 11 and 4 replies with either terminator', () => {
    const replies = '\x1b]11;rgb:0a0a/0a0a/0a0a\x1b\\\x1b]10;rgb:d4d4/d4d4/d4d4\x07\x1b]4;0;rgb:2e2e/3434/3636\x1b\\';
    expect(withoutColorReplies(replies)).toBe('');
  });

  it('keeps keys and other replies', () => {
    const input = 'ab\r\x1b[1;1R\x1b[?62;22c\x1b[I\x1b]52;c;aGk=\x07';
    expect(withoutColorReplies(input)).toBe(input);
  });
});
