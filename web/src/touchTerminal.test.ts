import { describe, expect, it, vi } from 'vitest';
import type { Terminal as XTerm } from '@xterm/xterm';
import { attachTouchGestures, hapticTick, wordBounds } from './touchTerminal';

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

describe('attachTouchGestures swipe', () => {
  it('sends the wheel from the finger position, so mouse-tracking TUIs report the finger cell', () => {
    const wheels: { clientX: number; clientY: number }[] = [];
    class FakeWheel {
      static DOM_DELTA_LINE = 1;
      constructor(
        public type: string,
        init: { clientX: number; clientY: number },
      ) {
        wheels.push(init);
      }
    }
    const box = { left: 0, top: 0, width: 500, height: 300 };
    const screen = { getBoundingClientRect: () => box, dispatchEvent: () => true };
    const handlers: Record<string, (e: unknown) => void> = {};
    const el = {
      style: { setProperty: () => {} },
      append: () => {},
      addEventListener: (type: string, fn: (e: unknown) => void) => (handlers[type] = fn),
      removeEventListener: () => {},
      getBoundingClientRect: () => box,
    };
    const none = { dispose: () => {} };
    const xterm = {
      element: { querySelector: () => screen },
      cols: 50,
      rows: 30,
      buffer: { active: { viewportY: 0 } },
      onSelectionChange: () => none,
      onScroll: () => none,
      onResize: () => none,
      clearSelection: () => {},
      hasSelection: () => false,
    } as unknown as XTerm;
    vi.stubGlobal('window', { matchMedia: () => ({ matches: true }), setTimeout: () => 0 });
    vi.stubGlobal('document', { createElement: () => ({ style: {}, contains: () => false, remove: () => {} }) });
    vi.stubGlobal('getComputedStyle', () => ({ position: 'relative' }));
    vi.stubGlobal('WheelEvent', FakeWheel);
    attachTouchGestures(xterm, el as unknown as HTMLElement, () => {});
    const touch = (x: number, y: number) => ({ touches: [{ clientX: x, clientY: y }], preventDefault: () => {}, target: null });
    handlers.touchstart(touch(230, 100));
    handlers.touchmove(touch(230, 130));
    handlers.touchmove(touch(230, 160));
    vi.unstubAllGlobals();
    expect(wheels.length).toBeGreaterThan(0);
    expect(wheels.every((w) => w.clientX === 230 && w.clientY === 160)).toBe(true);
  });
});
