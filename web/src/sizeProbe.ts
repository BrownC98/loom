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

export function probeSize(box: HTMLElement, fontFamily: string, fontSize: number): Resize | null {
  probe ??= makeProbe();
  const { xterm, fit, host } = probe;
  if (xterm.options.fontFamily !== fontFamily) xterm.options.fontFamily = fontFamily;
  if (xterm.options.fontSize !== fontSize) xterm.options.fontSize = fontSize;
  const style = getComputedStyle(box);
  host.style.width = style.width;
  host.style.height = style.height;
  const size = fit.proposeDimensions();
  return size && size.cols > 0 && size.rows > 0 ? { cols: size.cols, rows: size.rows } : null;
}
