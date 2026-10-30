import { createContext, useContext, useEffect, useRef, useState, type DragEvent, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import { Terminal as XTerm, type ITheme } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { withoutColorReplies } from './colorReplies';
import { IME_KEY_CODE, watchComposition } from './composition';
import { attachPty, PASTE_LIMIT_BYTES, savePastedFile } from './api';
import { droppedFiles, MB, quotePath } from './attachments';
import { offerInput } from './nodeInputs';
import type { NodeId } from './gen/NodeId';
import type { PtyRequest } from './gen/PtyRequest';
import type { Resize } from './gen/Resize';
import type { TerminalFont } from './gen/TerminalFont';
import i18next from './i18n';
import { openLink } from './openLink';
import { commandKey } from './platform';
import { findKey, navDirection, paneKey, type Direction } from './keys';
import { DEFAULT_TERMINAL_THEME, TERMINAL_THEMES } from './terminalThemes';
import { attachTouchGestures, type TouchSelection } from './touchTerminal';

const SHIFT_ENTER_INPUT = '\n';
function isShiftEnter(e: KeyboardEvent | ReactKeyboardEvent): boolean {
  return e.key === 'Enter' && e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey;
}

function isPlainEnter(e: KeyboardEvent): boolean {
  return e.key === 'Enter' && e.keyCode !== IME_KEY_CODE && !e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey;
}

function plainCommand(e: KeyboardEvent, code: string): boolean {
  return commandKey(e) && !e.shiftKey && !e.altKey && e.code === code;
}
export type XtermFontValue = { family: string; size: number };
export const DEFAULT_TERMINAL_FONT: XtermFontValue = { family: '"Jetendard", "Loom Symbols", monospace', size: 12 };
export const FONT_SIZE_RANGE = { min: 10, max: 24 } as const;

export function activeFont(saved: TerminalFont): XtermFontValue {
  return { family: saved.family?.trim() || DEFAULT_TERMINAL_FONT.family, size: saved.size ?? DEFAULT_TERMINAL_FONT.size };
}

function loadFont(family: string): Promise<unknown> {
  const unquote = (name: string) => name.trim().replace(/^["']|["']$/g, '');
  const names = family.split(',').map(unquote);
  return Promise.allSettled([...document.fonts].filter((f) => names.includes(unquote(f.family))).map((f) => f.load()));
}

type Point = { clientX: number; clientY: number };
type MouseService = {
  getCoords: (event: Point, element: HTMLElement, ...rest: never[]) => unknown;
  getMouseReportCoords: (event: Point, element: HTMLElement) => unknown;
};

function unscaleMouse(xterm: XTerm) {
  const mouse = (xterm as unknown as { _core?: { _mouseService?: MouseService } })._core?._mouseService;
  if (!mouse?.getCoords || !mouse.getMouseReportCoords) return;
  const unscale = (event: Point, element: HTMLElement): Point => {
    const rect = element.getBoundingClientRect();
    const scale = element.offsetWidth > 0 ? rect.width / element.offsetWidth : 1;
    if (scale === 1 || !Number.isFinite(scale) || scale <= 0) return event;
    return { clientX: rect.left + (event.clientX - rect.left) / scale, clientY: rect.top + (event.clientY - rect.top) / scale };
  };
  const { getCoords, getMouseReportCoords } = mouse;
  mouse.getCoords = (event, element, ...rest) => getCoords.call(mouse, unscale(event, element), element, ...rest);
  mouse.getMouseReportCoords = (event, element) => getMouseReportCoords.call(mouse, unscale(event, element), element);
}

const hasFiles = (e: DragEvent) => e.dataTransfer.types.includes('Files');

async function pasteDropped(xterm: XTerm, data: DataTransfer, node: NodeId | null) {
  const { files, errors } = droppedFiles(data);
  const saved = await Promise.allSettled(files.map((file) => savePastedFile(file, node)));
  const paths = saved.flatMap((r) => (r.status === 'fulfilled' ? [quotePath(r.value)] : []));
  errors.push(...saved.flatMap((r) => (r.status === 'rejected' ? [(r.reason as Error).message] : [])));
  if (errors.length) xterm.write(`\r\n${errors.join('\r\n')}\r\n`);
  if (paths.length) xterm.paste(`${paths.join(' ')} `);
}

function copySelection(xterm: XTerm) {
  void navigator.clipboard.writeText(xterm.getSelection()).catch((err: Error) => console.error(err));
  xterm.clearSelection();
}

function focusSoon(xterm: XTerm) {
  requestAnimationFrame(() => {
    if (!(document.activeElement instanceof HTMLInputElement)) xterm.focus();
  });
}

export const XtermTheme = createContext<ITheme>(TERMINAL_THEMES[DEFAULT_TERMINAL_THEME.dark]);
export const XtermFont = createContext<XtermFontValue>(DEFAULT_TERMINAL_FONT);

export type SizeHold = { claim: (size: Resize) => PtyRequest; whenHidden: PtyRequest | null };
export const keepSize: SizeHold = { claim: (size) => ({ resize: size }), whenHidden: null };
export const borrowSize: SizeHold = { claim: (size) => ({ borrow: size }), whenHidden: 'giveBack' };

type TerminalProps = {
  src: string;
  node: NodeId | null;
  focused: boolean;
  focusAsk?: number;
  onFocus: () => void;
  onNavigate: (dir: Direction) => void;
  onSender?: (send: ((data: string) => void) | null) => void;
  draw?: Draw;
  hold?: SizeHold;
};

export type Draw = 'live' | 'slow' | 'hidden';
const SLOW_DRAW_MS = 250;
const slowFlushes = new Set<() => void>();
let slowTimer: ReturnType<typeof setInterval> | undefined;
function holdSlow(flush: () => void): () => void {
  slowFlushes.add(flush);
  slowTimer ??= setInterval(() => slowFlushes.forEach((f) => f()), SLOW_DRAW_MS);
  return () => {
    slowFlushes.delete(flush);
    if (slowFlushes.size === 0) {
      clearInterval(slowTimer);
      slowTimer = undefined;
    }
  };
}

function boxTransform(xterm: XTerm, box: HTMLElement): string | null {
  const el = xterm.element;
  if (!el) return null;
  const width = el.offsetWidth;
  const height = el.offsetHeight;
  if (width === 0 || height === 0 || box.clientWidth === 0 || box.clientHeight === 0) return null;
  const fit = Math.min(box.clientWidth / width, box.clientHeight / height);
  if (fit < 1) return `translateX(${(box.clientWidth - width * fit) / 2}px) scale(${fit})`;
  const dpr = window.devicePixelRatio || 1;
  const at = box.getBoundingClientRect();
  const zoom = box.offsetWidth > 0 ? at.width / box.offsetWidth : 1;
  const snap = (v: number) => Math.round(v * dpr) / dpr;
  const left = (snap(at.left + ((box.clientWidth - width) / 2) * zoom) - at.left) / zoom;
  return `translate(${left}px, ${(snap(at.top) - at.top) / zoom}px)`;
}

function scaleToBox(xterm: XTerm, box: HTMLElement) {
  const transform = boxTransform(xterm, box);
  if (transform !== null && xterm.element) xterm.element.style.transform = transform;
}

const shown = new Map<HTMLElement, XTerm>();

export function realignTerminals() {
  const moves = [...shown].map(([box, xterm]) => [xterm.element, boxTransform(xterm, box)] as const);
  for (const [el, transform] of moves) if (el && transform !== null) el.style.transform = transform;
}

export function SizingTerminal(props: TerminalProps) {
  const box = useRef<HTMLDivElement>(null);
  const term = useRef<XTerm | null>(null);
  const latest = useRef(props);
  latest.current = props;
  const theme = useContext(XtermTheme);
  const latestTheme = useRef(theme);
  latestTheme.current = theme;
  const font = useContext(XtermFont);
  const latestFont = useRef(font);
  latestFont.current = font;
  const redecide = useRef<() => void>(() => {});
  const reflush = useRef<() => void>(() => {});
  const [dragging, setDragging] = useState<number | null>(null);
  const [picked, setPicked] = useState<TouchSelection | null>(null);

  useEffect(() => {
    let disposed = false;
    let cleanup = () => {};
    loadFont(latestFont.current.family).then(() => {
      if (disposed || !box.current) return;
      cleanup = mount(box.current);
    });
    return () => {
      disposed = true;
      cleanup();
    };

    function mount(el: HTMLDivElement) {
      const xterm = new XTerm({
        fontFamily: latestFont.current.family,
        fontSize: latestFont.current.size,
        lineHeight: 1,
        theme: latestTheme.current,
        linkHandler: { allowNonHttpProtocols: true, activate: (_event, uri) => openLink(uri, latest.current.node) },
      });
      const fitAddon = new FitAddon();
      xterm.loadAddon(fitAddon);
      xterm.open(el);
      unscaleMouse(xterm);
      term.current = xterm;
      const composition = watchComposition(xterm.textarea ?? el);
      let enterHeld = false;
      const replayed = new WeakSet<KeyboardEvent>();
      const rescale = () => scaleToBox(xterm, el);
      const propose = () => {
        const proposed = fitAddon.proposeDimensions();
        return proposed && proposed.cols > 0 && proposed.rows > 0 ? { cols: proposed.cols, rows: proposed.rows } : null;
      };
      const sendSize = (size: Resize) => link.sendRequest((latest.current.hold ?? keepSize).claim(size));
      const decide = () => {
        const size = propose();
        if (size) sendSize(size);
      };
      const claim = () => {
        const size = propose();
        if (size && (size.cols !== xterm.cols || size.rows !== xterm.rows)) sendSize(size);
      };
      const onVisibility = () => {
        const back = (latest.current.hold ?? keepSize).whenHidden;
        if (document.visibilityState === 'visible') claim();
        else if (back) link.sendRequest(back);
      };
      redecide.current = () => {
        decide();
        rescale();
      };
      const held: (() => void)[] = [];
      let replaying = 0;
      let settled = false;
      const flush = () => {
        for (const step of held.splice(0)) step();
      };
      const feed = (step: () => void) => {
        if ((latest.current.draw ?? 'live') === 'live') {
          flush();
          step();
        } else held.push(step);
      };
      const unhold = holdSlow(() => {
        if (latest.current.draw === 'slow') flush();
      });
      reflush.current = flush;
      let ended = false;
      const link = attachPty(props.src, {
        onAttach: () => {
          ended = false;
          held.length = 0;
          settled = false;
          xterm.reset();
          decide();
        },
        onBacklog: (bytes) =>
          feed(() => {
            if (!settled) return xterm.write(bytes);
            replaying += 1;
            xterm.write(bytes, () => (replaying -= 1));
          }),
        onOutput: (bytes) => feed(() => xterm.write(bytes)),
        onNotice: (notice) => {
          if (notice === 'gone') {
            ended = true;
            feed(() => xterm.write(`${i18next.t('terminal.gone')}\r\n`));
          }
          else if (notice === 'settled')
            feed(() => {
              settled = true;
            });
          else feed(() => xterm.resize(notice.size.cols, notice.size.rows));
        },
      });
      xterm.attachCustomKeyEventHandler((e) => {
        if (e.type === 'keydown') claim();
        const dir = navDirection(e);
        if (dir) {
          if (e.type === 'keydown') latest.current.onNavigate(dir);
          e.preventDefault();
          return false;
        }
        if (isPlainEnter(e) && !replayed.has(e)) {
          const later = () => {
            enterHeld = false;
            for (const type of ['keydown', 'keyup']) {
              const again = new KeyboardEvent(type, { key: e.key, code: e.code, keyCode: e.keyCode, bubbles: true, cancelable: true });
              replayed.add(again);
              xterm.textarea?.dispatchEvent(again);
            }
          };
          if (e.type === 'keydown' && composition.afterText(later)) enterHeld = true;
          if (enterHeld) {
            e.preventDefault();
            return false;
          }
        }
        if (isShiftEnter(e) && !e.isComposing) {
          if (e.type === 'keydown') setTimeout(() => link.sendInput(SHIFT_ENTER_INPUT));
          e.preventDefault();
          return false;
        }
        if (paneKey(e) || findKey(e)) return false;
        if (plainCommand(e, 'KeyC') && xterm.hasSelection()) {
          if (e.type === 'keydown') copySelection(xterm);
          return false;
        }
        return !plainCommand(e, 'KeyV');
      });
      const pasteImage = (e: ClipboardEvent) => {
        if (!e.clipboardData || e.clipboardData.getData('text/plain')) return;
        const image = [...e.clipboardData.files].find((file) => file.type.startsWith('image/'));
        if (!image) return;
        e.stopPropagation();
        e.preventDefault();
        savePastedFile(image, latest.current.node).then(
          (path) => xterm.paste(path),
          (err: Error) => xterm.write(`\r\n${err.message}\r\n`),
        );
      };
      el.addEventListener('paste', pasteImage, true);
      const input = xterm.onData((data) => {
        const sent = replaying === 0 ? data : withoutColorReplies(data);
        if (sent) link.sendInput(sent);
      });
      latest.current.onSender?.((data) => {
        claim();
        link.sendInput(data);
      });
      const node = latest.current.node;
      const unoffer =
        node === null
          ? () => {}
          : offerInput(node, {
              state: () => (ended ? 'gone' : link.connected() ? 'open' : 'offline'),
              paste: (text) => xterm.paste(text),
              enter: () => {
                claim();
                link.sendInput('\r');
              },
            });
      const detachTouch = attachTouchGestures(xterm, el, (at) => {
        const box = el.getBoundingClientRect();
        setPicked({ x: at.x - box.left, y: at.y - box.top });
      });
      const unpick = xterm.onSelectionChange(() => xterm.hasSelection() || setPicked(null));
      const resize = xterm.onResize(() => requestAnimationFrame(rescale));
      xterm.textarea?.addEventListener('focus', () => {
        claim();
        latest.current.onFocus();
      });
      window.addEventListener('focus', claim);
      document.addEventListener('visibilitychange', onVisibility);
      const observer = new ResizeObserver(() => redecide.current());
      shown.set(el, xterm);
      observer.observe(el);
      if (latest.current.focused) focusSoon(xterm);
      return () => {
        unhold();
        composition.dispose();
        reflush.current = () => {};
        observer.disconnect();
        window.removeEventListener('focus', claim);
        document.removeEventListener('visibilitychange', onVisibility);
        shown.delete(el);
        el.removeEventListener('paste', pasteImage, true);
        input.dispose();
        resize.dispose();
        unpick.dispose();
        detachTouch();
        latest.current.onSender?.(null);
        unoffer();
        link.close();
        xterm.dispose();
        term.current = null;
        redecide.current = () => {};
      };
    }
  }, [props.src]);

  useEffect(() => {
    if (term.current) term.current.options.theme = theme;
  }, [theme]);

  useEffect(() => {
    if ((props.draw ?? 'live') !== 'hidden') reflush.current();
  }, [props.draw]);

  useEffect(() => {
    let disposed = false;
    loadFont(font.family).then(() => {
      const xterm = term.current;
      if (disposed || !xterm) return;
      xterm.options.fontFamily = font.family;
      xterm.options.fontSize = font.size;
      requestAnimationFrame(() => redecide.current());
    });
    return () => {
      disposed = true;
    };
  }, [font.family, font.size]);

  useEffect(() => {
    if (props.focused && term.current) focusSoon(term.current);
    else term.current?.blur();
  }, [props.focused, props.focusAsk]);

  const onDragOver = (e: DragEvent) => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    e.stopPropagation();
    e.dataTransfer.dropEffect = 'copy';
    setDragging([...e.dataTransfer.items].filter((item) => item.kind === 'file').length);
  };
  const onDragLeave = (e: DragEvent) => {
    if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDragging(null);
  };
  const onDrop = (e: DragEvent) => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    e.stopPropagation();
    setDragging(null);
    const xterm = term.current;
    if (!xterm) return;
    xterm.focus();
    void pasteDropped(xterm, e.dataTransfer, props.node);
  };

  const copyPicked = () => {
    if (term.current) copySelection(term.current);
  };

  return (
    <div className="xterm-box" style={{ background: theme.background }} onDragOver={onDragOver} onDragLeave={onDragLeave} onDrop={onDrop}>
      <div ref={box} />
      {picked && (
        <button type="button" className="copy-picked" style={{ left: picked.x, top: picked.y + 28 }} onClick={copyPicked}>
          {i18next.t('terminal.copy')}
        </button>
      )}
      {dragging !== null && (
        <div className="drop-cover">
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M12 3v12M7 10l5 5 5-5M5 21h14" />
          </svg>
          <span>{i18next.t('terminal.drop')}</span>
          <span className="sub">{i18next.t('terminal.dropCount', { count: dragging, limit: PASTE_LIMIT_BYTES / MB })}</span>
        </div>
      )}
    </div>
  );
}
