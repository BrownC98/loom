import type { IBufferCell, IBufferLine, ITheme, Terminal } from '@xterm/xterm';

const ANSI = ['black', 'red', 'green', 'yellow', 'blue', 'magenta', 'cyan', 'white', 'brightBlack', 'brightRed', 'brightGreen', 'brightYellow', 'brightBlue', 'brightMagenta', 'brightCyan', 'brightWhite'] as const;
const CUBE = [0, 95, 135, 175, 215, 255];
const BRIGHT_OFFSET = 8;
type Run = { style: string; text: string };
const drawn = new WeakMap<Element, { key: string; texts: Text[] }>();

const COLOR_SPAN = 2 ** 26;

function flagsOf(cell: IBufferCell): number {
  return (cell.isBold() && 1) | (cell.isItalic() && 2) | (cell.isDim() && 4) | (cell.isUnderline() && 8) | (cell.isInverse() && 16) | (cell.isInvisible() && 32) | (cell.isStrikethrough() && 64) | (cell.isOverline() && 128);
}

function hex(rgb: number): string {
  return `#${rgb.toString(16).padStart(6, '0')}`;
}

function palette(index: number, theme: ITheme): string | undefined {
  if (index < ANSI.length) return theme[ANSI[index]];
  if (index >= 232) {
    const gray = 8 + (index - 232) * 10;
    return hex(gray * 0x10101);
  }
  const cube = index - 16;
  return hex((CUBE[Math.floor(cube / 36)] << 16) | (CUBE[Math.floor(cube / 6) % 6] << 8) | CUBE[cube % 6]);
}

type Color = { rgb: boolean; indexed: boolean; value: number };

function colorOf(color: Color, theme: ITheme, bright: boolean): string | undefined {
  if (color.rgb) return hex(color.value);
  if (!color.indexed) return undefined;
  return palette(bright && color.value < BRIGHT_OFFSET ? color.value + BRIGHT_OFFSET : color.value, theme);
}

function styleOf(cell: IBufferCell, theme: ITheme): string {
  let front: Color = { rgb: cell.isFgRGB(), indexed: cell.isFgPalette(), value: cell.getFgColor() };
  let back: Color = { rgb: cell.isBgRGB(), indexed: cell.isBgPalette(), value: cell.getBgColor() };
  if (cell.isInverse()) [front, back] = [back, front];
  let fg = colorOf(front, theme, cell.isBold() !== 0);
  let bg = colorOf(back, theme, false);
  if (cell.isInverse()) {
    fg ??= theme.background;
    bg ??= theme.foreground;
  }
  const parts: string[] = [];
  if (cell.isInvisible()) parts.push('color:transparent');
  else if (cell.isDim()) parts.push(`color:color-mix(in srgb, ${fg ?? theme.foreground} 50%, transparent)`);
  else if (fg) parts.push(`color:${fg}`);
  if (bg) parts.push(`background:${bg}`);
  if (cell.isBold()) parts.push('font-weight:bold');
  if (cell.isItalic()) parts.push('font-style:italic');
  const lines = [cell.isUnderline() && 'underline', cell.isStrikethrough() && 'line-through', cell.isOverline() && 'overline'].filter(Boolean);
  if (lines.length) parts.push(`text-decoration:${lines.join(' ')}`);
  return parts.join(';');
}

function rowRuns(line: IBufferLine | undefined, cols: number, theme: ITheme, cell: IBufferCell): Run[] {
  const runs: Run[] = [];
  if (!line) return runs;
  let style = '';
  let start = 0;
  const close = (end: number) => {
    if (end > start) runs.push({ style, text: line.translateToString(!style && end === cols, start, end) });
    start = end;
  };
  let colors = 0;
  let flags = 0;
  for (let x = 0; x < cols; x++) {
    if (!line.getCell(x, cell)) break;
    const plain = cell.isAttributeDefault();
    if (plain && !style) continue;
    const nextColors = plain ? 0 : (cell.getFgColorMode() | cell.getFgColor()) * COLOR_SPAN + (cell.getBgColorMode() | cell.getBgColor());
    const nextFlags = plain ? 0 : flagsOf(cell);
    if (style && nextColors === colors && nextFlags === flags) continue;
    const next = plain ? '' : styleOf(cell, theme);
    colors = nextColors;
    flags = nextFlags;
    if (next === style) continue;
    close(x);
    style = next;
  }
  close(cols);
  return runs;
}

function drawRow(row: Element, runs: Run[]) {
  const key = runs.map((run) => run.style).join('|');
  const old = drawn.get(row);
  if (old?.key === key) {
    runs.forEach((run, i) => {
      if (old.texts[i].data !== run.text) old.texts[i].data = run.text;
    });
    return;
  }
  const texts = runs.map((run) => document.createTextNode(run.text));
  row.replaceChildren(
    ...runs.map((run, i) => {
      if (!run.style) return texts[i];
      const span = document.createElement('span');
      span.setAttribute('style', run.style);
      span.append(texts[i]);
      return span;
    }),
  );
  drawn.set(row, { key, texts });
}

export function paintSnapshot(xterm: Terminal, pre: HTMLElement, theme: ITheme) {
  const { cols, rows } = xterm;
  const buffer = xterm.buffer.active;
  const cell = buffer.getNullCell();
  pre.style.fontFamily = xterm.options.fontFamily ?? '';
  pre.style.fontSize = `${xterm.options.fontSize}px`;
  pre.style.color = theme.foreground ?? '';
  pre.style.width = `${cols}ch`;
  while (pre.childElementCount > rows) pre.lastElementChild?.remove();
  while (pre.childElementCount < rows) pre.append(document.createElement('div'));
  for (let y = 0; y < rows; y++) drawRow(pre.children[y], rowRuns(buffer.getLine(buffer.viewportY + y), cols, theme, cell));
}
