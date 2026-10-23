import type { Terminal as XTerm } from '@xterm/xterm';

export const LONG_PRESS_MS = 450;
export const SLOP_PX = 10;

const WHEEL_NOTCH = 120;

export function hapticTick() {
  const chrome = (window as unknown as { loomChrome?: { haptic?: () => void } }).loomChrome;
  if (chrome?.haptic) chrome.haptic();
  else navigator.vibrate?.(15);
}

export type TouchSelection = { x: number; y: number };

type Cell = { col: number; row: number };

function cellAt(xterm: XTerm, screen: HTMLElement, x: number, y: number): Cell {
  const rect = screen.getBoundingClientRect();
  const col = Math.min(xterm.cols - 1, Math.max(0, Math.floor(((x - rect.left) / rect.width) * xterm.cols)));
  const row = Math.min(xterm.rows - 1, Math.max(0, Math.floor(((y - rect.top) / rect.height) * xterm.rows)));
  return { col, row: xterm.buffer.active.viewportY + row };
}

const before = (a: Cell, b: Cell) => a.row < b.row || (a.row === b.row && a.col < b.col);

function selectBetween(xterm: XTerm, a: Cell, b: Cell) {
  const [from, to] = before(b, a) ? [b, a] : [a, b];
  xterm.select(from.col, from.row, (to.row - from.row) * xterm.cols + to.col - from.col + 1);
}

export function wordBounds(chars: readonly string[], col: number, separators: string): { start: number; end: number } {
  const inWord = (c: string) => c === '' || !(c === ' ' || separators.includes(c));
  if (!inWord(chars[col])) return { start: col, end: col };
  let start = col;
  while (start > 0 && inWord(chars[start - 1])) start--;
  let end = col;
  while (end + 1 < chars.length && inWord(chars[end + 1])) end++;
  return { start, end };
}

function wordAt(xterm: XTerm, at: Cell): { from: Cell; to: Cell } {
  const line = xterm.buffer.active.getLine(at.row);
  if (!line) return { from: at, to: at };
  const chars = Array.from({ length: xterm.cols }, (_, x) => {
    const cell = line.getCell(x);
    if (!cell) return ' ';
    return cell.getWidth() === 0 ? '' : cell.getChars() || ' ';
  });
  const { start, end } = wordBounds(chars, at.col, xterm.options.wordSeparator ?? '');
  return { from: { col: start, row: at.row }, to: { col: end, row: at.row } };
}

export function attachTouchGestures(xterm: XTerm, el: HTMLElement, onSelect: (at: TouchSelection) => void): () => void {
  const screen = xterm.element?.querySelector<HTMLElement>('.xterm-screen');
  if (!screen || !window.matchMedia('(pointer: coarse)').matches) return () => {};
  el.style.touchAction = 'none';
  el.style.setProperty('-webkit-touch-callout', 'none');
  el.style.userSelect = 'none';
  if (getComputedStyle(el).position === 'static') el.style.position = 'relative';
  const onMenu = (e: Event) => {
    e.preventDefault();
    e.stopPropagation();
  };

  type End = 'from' | 'to';
  type State =
    | { mode: 'press' | 'scroll' | 'select'; x: number; y: number; anchor: Cell; word: { from: Cell; to: Cell }; carry: number; timer: number; hadSelection: boolean }
    | { mode: 'handle'; end: End; dx: number; dy: number };
  let state: State | null = null;
  let picked: { from: Cell; to: Cell } | null = null;

  const handles = { from: document.createElement('div'), to: document.createElement('div') };
  handles.from.className = 'touch-handle from';
  handles.to.className = 'touch-handle to';
  el.append(handles.from, handles.to);

  const cellBox = () => {
    const rect = screen.getBoundingClientRect();
    return { rect, width: rect.width / xterm.cols, height: rect.height / xterm.rows };
  };
  const edge = (cell: Cell, end: End) => {
    const { rect, width, height } = cellBox();
    return { x: rect.left + (cell.col + (end === 'to' ? 1 : 0)) * width, y: rect.top + (cell.row - xterm.buffer.active.viewportY + 1) * height };
  };
  const place = () => {
    const box = el.getBoundingClientRect();
    for (const end of ['from', 'to'] as const) {
      const handle = handles[end];
      const row = picked ? picked[end].row - xterm.buffer.active.viewportY : -1;
      if (!picked || row < 0 || row >= xterm.rows) {
        handle.style.display = 'none';
        continue;
      }
      const at = edge(picked[end], end);
      handle.style.display = 'block';
      handle.style.left = `${at.x - box.left}px`;
      handle.style.top = `${at.y - box.top}px`;
    }
  };
  const pick = (from: Cell, to: Cell) => {
    picked = before(to, from) ? { from: to, to: from } : { from, to };
    selectBetween(xterm, picked.from, picked.to);
    place();
  };
  const unpick = xterm.onSelectionChange(() => {
    if (xterm.hasSelection()) return;
    picked = null;
    place();
  });
  const scrolled = xterm.onScroll(place);
  const resized = xterm.onResize(() => {
    const keep = picked;
    place();
    if (!keep) return;
    requestAnimationFrame(() => {
      if (xterm.hasSelection()) return;
      pick(keep.from, keep.to);
      onSelect(edge(keep.to, 'to'));
    });
  });

  const wheel = (lines: number, clientX: number, clientY: number) => {
    const event = new WheelEvent('wheel', { clientX, clientY, deltaY: lines, deltaMode: WheelEvent.DOM_DELTA_LINE, bubbles: true, cancelable: true });
    Object.defineProperty(event, 'wheelDeltaY', { value: -lines * WHEEL_NOTCH });
    screen.dispatchEvent(event);
  };

  const tap = (clientX: number, clientY: number) => {
    const target = document.elementFromPoint(clientX, clientY) ?? screen;
    for (const type of ['mousemove', 'mousedown', 'mouseup', 'click'])
      target.dispatchEvent(new MouseEvent(type, { clientX, clientY, button: 0, buttons: type === 'mousedown' ? 1 : 0, bubbles: true, cancelable: true, view: window }));
  };

  const onStart = (e: TouchEvent) => {
    if (state?.mode !== 'handle' && state) clearTimeout(state.timer);
    if (e.touches.length !== 1) return void (state = null);
    e.preventDefault();
    const { clientX: x, clientY: y } = e.touches[0];
    const end = (Object.keys(handles) as End[]).find((k) => handles[k].contains(e.target as Node));
    if (end && picked) {
      const { width, height } = cellBox();
      const at = edge(picked[end], end);
      state = { mode: 'handle', end, dx: at.x - (end === 'to' ? width / 2 : -width / 2) - x, dy: at.y - height / 2 - y };
      return;
    }
    const anchor = cellAt(xterm, screen, x, y);
    const next: State = { mode: 'press', x, y, anchor, word: { from: anchor, to: anchor }, carry: 0, timer: 0, hadSelection: xterm.hasSelection() };
    next.timer = window.setTimeout(() => {
      next.mode = 'select';
      next.word = wordAt(xterm, next.anchor);
      xterm.blur();
      pick(next.word.from, next.word.to);
      hapticTick();
    }, LONG_PRESS_MS);
    xterm.clearSelection();
    state = next;
  };

  const onMove = (e: TouchEvent) => {
    if (!state || e.touches.length !== 1) return;
    const { clientX: x, clientY: y } = e.touches[0];
    if (state.mode === 'handle') {
      e.preventDefault();
      if (!picked) return;
      const at = cellAt(xterm, screen, x + state.dx, y + state.dy);
      const other: End = state.end === 'from' ? 'to' : 'from';
      if (state.end === 'from' ? before(picked.to, at) : before(at, picked.from)) state.end = other;
      return state.end === 'from' ? pick(at, picked.to) : pick(picked.from, at);
    }
    if (state.mode === 'press') {
      if (Math.hypot(x - state.x, y - state.y) < SLOP_PX) return;
      clearTimeout(state.timer);
      state.mode = 'scroll';
      state.y = y;
    }
    e.preventDefault();
    if (state.mode === 'select') {
      const at = cellAt(xterm, screen, x, y);
      const { from, to } = state.word;
      return before(at, from) ? pick(at, to) : pick(from, before(to, at) ? at : to);
    }
    const cellHeight = screen.getBoundingClientRect().height / xterm.rows;
    state.carry += (state.y - y) / cellHeight;
    state.y = y;
    const lines = Math.trunc(state.carry);
    if (lines === 0) return;
    state.carry -= lines;
    wheel(lines, x, y);
  };

  const onEnd = (e: TouchEvent) => {
    if (!state) return;
    if (state.mode !== 'handle') clearTimeout(state.timer);
    const touch = e.changedTouches[0];
    if ((state.mode === 'select' || state.mode === 'handle') && xterm.hasSelection()) onSelect({ x: touch.clientX, y: touch.clientY });
    if (e.type === 'touchend' && state.mode === 'press' && !state.hadSelection) tap(touch.clientX, touch.clientY);
    state = null;
  };

  el.addEventListener('touchstart', onStart, { passive: false });
  el.addEventListener('touchmove', onMove, { passive: false });
  el.addEventListener('touchend', onEnd, { passive: false });
  el.addEventListener('touchcancel', onEnd, { passive: false });
  el.addEventListener('contextmenu', onMenu, { capture: true });
  return () => {
    if (state && state.mode !== 'handle') clearTimeout(state.timer);
    unpick.dispose();
    scrolled.dispose();
    resized.dispose();
    handles.from.remove();
    handles.to.remove();
    el.removeEventListener('touchstart', onStart);
    el.removeEventListener('touchmove', onMove);
    el.removeEventListener('touchend', onEnd);
    el.removeEventListener('touchcancel', onEnd);
    el.removeEventListener('contextmenu', onMenu, { capture: true });
  };
}

type AppClipboard = { read: () => string };

export async function readClipboard(): Promise<string> {
  const app = (window as unknown as { loomClipboard?: AppClipboard }).loomClipboard;
  return app ? app.read() : navigator.clipboard.readText();
}
