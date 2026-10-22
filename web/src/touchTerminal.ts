import type { Terminal as XTerm } from '@xterm/xterm';

/** 이만큼 누르고 있으면 글자 고르기다. 안드로이드 기본 길게 누르기(500ms)보다 조금 짧다. */
const LONG_PRESS_MS = 450;
/** 손가락이 이보다 덜 움직였으면 아직 누른 자리다(스크롤·고르기로 가르는 문턱). */
const SLOP_PX = 10;

/** 휠 눈금 하나의 `wheelDeltaY`. */
const WHEEL_NOTCH = 120;

export type TouchSelection = { x: number; y: number };

type Cell = { col: number; row: number };

/** 칸 배율을 빼고 화면 좌표를 버퍼 칸으로 바꾼다. `.xterm-screen`의 화면 크기가 열·줄 수를 나눈 값이 칸 크기다. */
function cellAt(xterm: XTerm, screen: HTMLElement, x: number, y: number): Cell {
  const rect = screen.getBoundingClientRect();
  const col = Math.min(xterm.cols - 1, Math.max(0, Math.floor(((x - rect.left) / rect.width) * xterm.cols)));
  const row = Math.min(xterm.rows - 1, Math.max(0, Math.floor(((y - rect.top) / rect.height) * xterm.rows)));
  return { col, row: xterm.buffer.active.viewportY + row };
}

const before = (a: Cell, b: Cell) => a.row < b.row || (a.row === b.row && a.col < b.col);

/** 두 칸 사이(양 끝 포함)를 고른다. 거꾸로 끌어도 된다. */
function selectBetween(xterm: XTerm, a: Cell, b: Cell) {
  const [from, to] = before(b, a) ? [b, a] : [a, b];
  xterm.select(from.col, from.row, (to.row - from.row) * xterm.cols + to.col - from.col + 1);
}

/**
 * `col` 칸을 품은 낱말의 처음·끝 열. 빈칸이나 구분 글자 위면 그 칸 하나다.
 * `chars`는 칸마다의 글자다. 넓은 글자의 둘째 칸은 빈 문자열이라 앞 글자에 붙는다.
 */
export function wordBounds(chars: readonly string[], col: number, separators: string): { start: number; end: number } {
  const inWord = (c: string) => c === '' || !(c === ' ' || separators.includes(c));
  if (!inWord(chars[col])) return { start: col, end: col };
  let start = col;
  while (start > 0 && inWord(chars[start - 1])) start--;
  let end = col;
  while (end + 1 < chars.length && inWord(chars[end + 1])) end++;
  return { start, end };
}

/** 길게 누른 칸의 낱말. 줄이 없으면 그 칸 하나다. */
function wordAt(xterm: XTerm, at: Cell): { from: Cell; to: Cell } {
  const line = xterm.buffer.active.getLine(at.row);
  if (!line) return { from: at, to: at };
  // 쓴 적 없는 칸도 글자가 ''이다. 너비로 넓은 글자의 둘째 칸(너비 0)과 가른다.
  const chars = Array.from({ length: xterm.cols }, (_, x) => {
    const cell = line.getCell(x);
    if (!cell) return ' ';
    return cell.getWidth() === 0 ? '' : cell.getChars() || ' ';
  });
  const { start, end } = wordBounds(chars, at.col, xterm.options.wordSeparator ?? '');
  return { from: { col: start, row: at.row }, to: { col: end, row: at.row } };
}

/**
 * 폰 터미널 손가락 동작. 한 손가락 세로 쓸기는 xterm에 휠 입력으로 보낸다: 셸은 스크롤백이 움직이고,
 * 마우스 추적을 켠 TUI(Claude Code·less)는 휠을 받는다. 길게 누르면 폰 관례대로 그 낱말을 고르고, 끌면 낱말에서 손가락까지 늘어난다
 * (2026-10-23 사람 "모바일의 일반적 관례를 따르라 예를 들면 화면을 꾹 눌렀을 때 단어 단위로 이미 지정이 된다던가").
 * 짧게 누르면 아무것도 하지 않아 xterm의 포커스(키보드)가 그대로 열린다.
 * `onSelect`는 고르기가 끝난 손가락 자리를 알린다(복사 버튼 자리). 터치 기기에서만 건다.
 */
export function attachTouchGestures(xterm: XTerm, el: HTMLElement, onSelect: (at: TouchSelection) => void): () => void {
  const screen = xterm.element?.querySelector<HTMLElement>('.xterm-screen');
  if (!screen || !window.matchMedia('(pointer: coarse)').matches) return () => {};
  // 브라우저가 쓸기를 페이지 스크롤로 가져가면 touchmove를 막을 수 없다.
  el.style.touchAction = 'none';
  // Android는 약 500ms 길게 누르면 contextmenu를 내고 자기 글자 고르기를 시작하며 touchcancel을 보낸다. 그러면 우리 고르기가 끝나 끌어도 늘지 않았다(2026-10-22 사람 "스크롤은 되는데 텍스트 선택이 안됨", 실제 앱). 기본 메뉴와 고르기를 막는다.
  el.style.setProperty('-webkit-touch-callout', 'none');
  el.style.userSelect = 'none';
  const onMenu = (e: Event) => e.preventDefault();

  type State = { mode: 'press' | 'scroll' | 'select'; x: number; y: number; anchor: Cell; word: { from: Cell; to: Cell }; carry: number; timer: number };
  let state: State | null = null;

  const wheel = (lines: number) => {
    const event = new WheelEvent('wheel', { deltaY: lines, deltaMode: WheelEvent.DOM_DELTA_LINE, bubbles: true, cancelable: true });
    // 스크롤백 처리기(VS Code 스크롤 칸)는 `deltaY`가 아니라 옛 필드 `wheelDeltaY`를 읽는다(눈금 하나가 -120). 만든 이벤트는 그 값이 0이라 스크롤하지 않는다.
    Object.defineProperty(event, 'wheelDeltaY', { value: -lines * WHEEL_NOTCH });
    // 진짜 휠처럼 화면(`.xterm-screen`)에서 시작해 위로 올린다. 스크롤백 처리기는 위 `.xterm-scrollable-element`에, 마우스 모드 처리기는 `xterm.element`에 붙어 있다.
    screen.dispatchEvent(event);
  };

  const onStart = (e: TouchEvent) => {
    if (state) clearTimeout(state.timer);
    if (e.touches.length !== 1) return void (state = null);
    const { clientX: x, clientY: y } = e.touches[0];
    const anchor = cellAt(xterm, screen, x, y);
    const next: State = { mode: 'press', x, y, anchor, word: { from: anchor, to: anchor }, carry: 0, timer: 0 };
    next.timer = window.setTimeout(() => {
      next.mode = 'select';
      next.word = wordAt(xterm, next.anchor);
      selectBetween(xterm, next.word.from, next.word.to);
    }, LONG_PRESS_MS);
    // 새로 누르면 앞 고르기는 끝낸다.
    xterm.clearSelection();
    state = next;
  };

  const onMove = (e: TouchEvent) => {
    if (!state || e.touches.length !== 1) return;
    const { clientX: x, clientY: y } = e.touches[0];
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
      return before(at, from) ? selectBetween(xterm, at, to) : selectBetween(xterm, from, before(to, at) ? at : to);
    }
    // 손가락이 위로 가면 내용이 위로 가고, 나중 줄이 보인다.
    const cellHeight = screen.getBoundingClientRect().height / xterm.rows;
    state.carry += (state.y - y) / cellHeight;
    state.y = y;
    const lines = Math.trunc(state.carry);
    if (lines === 0) return;
    state.carry -= lines;
    wheel(lines);
  };

  const onEnd = (e: TouchEvent) => {
    if (!state) return;
    clearTimeout(state.timer);
    if (state.mode === 'select') {
      e.preventDefault();
      const touch = e.changedTouches[0];
      if (xterm.hasSelection()) onSelect({ x: touch.clientX, y: touch.clientY });
    }
    state = null;
  };

  el.addEventListener('touchstart', onStart, { passive: true });
  el.addEventListener('touchmove', onMove, { passive: false });
  el.addEventListener('touchend', onEnd, { passive: false });
  el.addEventListener('touchcancel', onEnd, { passive: false });
  el.addEventListener('contextmenu', onMenu, { capture: true });
  return () => {
    if (state) clearTimeout(state.timer);
    el.removeEventListener('touchstart', onStart);
    el.removeEventListener('touchmove', onMove);
    el.removeEventListener('touchend', onEnd);
    el.removeEventListener('touchcancel', onEnd);
    el.removeEventListener('contextmenu', onMenu, { capture: true });
  };
}

/** 안드로이드 앱이 PC 화면에만 붙이는 클립보드 읽기(`MainActivity`). WebView는 `navigator.clipboard.readText`를 막는다. */
type AppClipboard = { read: () => string };

export async function readClipboard(): Promise<string> {
  const app = (window as unknown as { loomClipboard?: AppClipboard }).loomClipboard;
  return app ? app.read() : navigator.clipboard.readText();
}
