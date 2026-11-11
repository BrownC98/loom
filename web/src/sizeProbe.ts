import { Terminal as XTerm } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import type { Resize } from './gen/Resize';

type Probe = { xterm: XTerm; fit: FitAddon; host: HTMLDivElement };
let probe: Probe | null = null;

function makeProbe(): Probe {
  const host = document.createElement('div');
  host.style.cssText = 'position:fixed;left:-10000px;top:0;visibility:hidden;contain:strict';
  document.body.append(host);
  const xterm = new XTerm({ lineHeight: 1 });
  const fit = new FitAddon();
  xterm.loadAddon(fit);
  xterm.open(host);
  return { xterm, fit, host };
}

const FIT_MIN_COLS = 2;
const FIT_MIN_ROWS = 1;

export function fitsTerminal(size: Resize | undefined | null): size is Resize {
  return Boolean(size && size.cols > FIT_MIN_COLS && size.rows > FIT_MIN_ROWS);
}

export function probeSize(box: HTMLElement, fontFamily: string, fontSize: number): Resize | null {
  if (box.clientWidth <= 0 || box.clientHeight <= 0) return null;
  probe ??= makeProbe();
  const { xterm, fit, host } = probe;
  if (xterm.options.fontFamily !== fontFamily) xterm.options.fontFamily = fontFamily;
  if (xterm.options.fontSize !== fontSize) xterm.options.fontSize = fontSize;
  host.style.width = `${box.clientWidth}px`;
  host.style.height = `${box.clientHeight}px`;
  const size = fit.proposeDimensions();
  return fitsTerminal(size) ? { cols: size.cols, rows: size.rows } : null;
}
