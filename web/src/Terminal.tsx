import { createContext, useContext, useEffect, useRef, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import { Terminal as XTerm, type ITheme } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import hangulCss from '@fontsource/nanum-gothic-coding/400.css?inline';
import { attachPty } from './api';
import type { TerminalFont } from './gen/TerminalFont';
import i18next from './i18n';
import { DEFAULT_TERMINAL_THEME, TERMINAL_THEMES } from './terminalThemes';

export type Direction = 'left' | 'right' | 'up' | 'down';

const NAV_KEYS: Record<string, Direction> = {
  ArrowLeft: 'left',
  ArrowRight: 'right',
  ArrowUp: 'up',
  ArrowDown: 'down',
};

/** `Ctrl+Shift+`방향키는 터미널 앱에 넘기지 않고 옆 카드 고르기에 쓴다(목업 「터미널에서 빠져나오기」). */
export function navDirection(e: KeyboardEvent | ReactKeyboardEvent): Direction | null {
  if (!e.ctrlKey || !e.shiftKey || e.altKey || e.metaKey) return null;
  return NAV_KEYS[e.key] ?? null;
}

/** 번들한 한글 글꼴. 원래 한글 폭(1em)이 JetBrains Mono 두 칸(7/6em)보다 좁아 xterm이 글자 사이에 틈을 둔다. 그래서 7/6배로 키운다. */
const HANGUL_FONT = 'Nanum Gothic Coding';
const HANGUL_SIZE_ADJUST = `${(700 / 6).toFixed(2)}%`;
const HANGUL_BLOCKS = [
  [0x1100, 0x11ff],
  [0x3130, 0x318f],
  [0xac00, 0xd7af],
] as const;

declare global {
  // 브라우저에는 있지만 TypeScript DOM 타입에 아직 없다(CSS Fonts 5).
  interface FontFaceDescriptors {
    sizeAdjust?: string;
  }
}

/**
 * 번들 CSS의 조각마다 한글 범위만 남긴 글꼴을 등록한다. 그 글꼴의 다른 글자(라틴, 상자 글자 `─│` 같은 기호)는 전각이라
 * 한 칸에 그리면 깨진다. 빠진 글자는 목록의 다음 글꼴이 그린다. CSS로 선언한 글꼴은 Chrome이 범위를 바꿔도 따르지 않아 새로 만든다.
 */
function registerHangulFont() {
  const sheet = new CSSStyleSheet();
  sheet.replaceSync(hangulCss);
  for (const rule of sheet.cssRules) {
    if (!(rule instanceof CSSFontFaceRule)) continue;
    const ranges = rule.style
      .getPropertyValue('unicode-range')
      .split(',')
      .flatMap((part) => {
        const [lo, hi = lo] = part.trim().slice(2).split('-').map((hex) => parseInt(hex, 16));
        return HANGUL_BLOCKS.filter(([first, last]) => lo <= last && hi >= first).map(([first, last]) => `U+${Math.max(lo, first).toString(16)}-${Math.min(hi, last).toString(16)}`);
      });
    if (ranges.length === 0) continue;
    document.fonts.add(new FontFace(HANGUL_FONT, rule.style.getPropertyValue('src'), { unicodeRange: ranges.join(','), sizeAdjust: HANGUL_SIZE_ADJUST }));
  }
}
registerHangulFont();

export type XtermFontValue = { family: string; size: number };
/** 기본 글꼴로 카드가 80열이다(`.card`의 폭). 근거: docs/design.md 「설정」. */
export const DEFAULT_TERMINAL_FONT: XtermFontValue = { family: `"JetBrains Mono", "${HANGUL_FONT}", monospace`, size: 12 };
/** Orca의 글꼴 크기 칸과 같은 범위다. */
export const FONT_SIZE_RANGE = { min: 10, max: 24 } as const;

/** 저장한 값이 없으면 기본값을 쓴다. */
export function activeFont(saved: TerminalFont): XtermFontValue {
  return { family: saved.family?.trim() || DEFAULT_TERMINAL_FONT.family, size: saved.size ?? DEFAULT_TERMINAL_FONT.size };
}

/** 목록에 있는 번들 글꼴을 모두 읽는다. xterm은 글자 폭을 처음 그릴 때 재 두므로, 늦게 읽힌 글꼴은 글자 사이를 틀리게 띄운다. */
function loadFont(family: string): Promise<unknown> {
  const unquote = (name: string) => name.trim().replace(/^["']|["']$/g, '');
  const names = family.split(',').map(unquote);
  return Promise.allSettled([...document.fonts].filter((f) => names.includes(unquote(f.family))).map((f) => f.load()));
}

/** 지금 모드의 터미널 테마. `App`이 설정과 모드로 정한다. */
export const XtermTheme = createContext<ITheme>(TERMINAL_THEMES[DEFAULT_TERMINAL_THEME.dark]);
/** 모든 터미널의 글꼴. `App`이 설정으로 정한다. */
export const XtermFont = createContext<XtermFontValue>(DEFAULT_TERMINAL_FONT);

/** `src`는 PTY WebSocket 경로다(`nodePty`·`loginPty`). */
export function Terminal(props: { src: string; focused: boolean; onFocus: () => void; onNavigate: (dir: Direction) => void }) {
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
  const fit = useRef<FitAddon | null>(null);

  useEffect(() => {
    let disposed = false;
    let cleanup = () => {};
    // 글꼴을 읽기 전에 열면 글자 폭을 잘못 재고 열 수가 틀린다.
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
        // 줄 높이가 1보다 크면 DOM 렌더러가 블록 문자 줄 사이에 틈을 남긴다(Claude 마스코트).
        // WebGL 렌더러는 틈이 없지만 브라우저가 컨텍스트 수를 제한해 카드가 많으면 깨진다.
        lineHeight: 1,
        theme: latestTheme.current,
      });
      const fitAddon = new FitAddon();
      xterm.loadAddon(fitAddon);
      xterm.open(el);
      term.current = xterm;
      fit.current = fitAddon;
      const link = attachPty(props.src, {
        onAttach: () => {
          // 다시 붙으면 backlog를 처음부터 받으므로 화면을 비운다.
          xterm.reset();
          link.sendResize({ cols: xterm.cols, rows: xterm.rows });
          if (latest.current.focused) xterm.focus();
        },
        onOutput: (bytes) => xterm.write(bytes),
        onNotice: (notice) => {
          switch (notice) {
            case 'gone':
              xterm.write(`${i18next.t('terminal.gone')}\r\n`);
          }
        },
      });
      xterm.attachCustomKeyEventHandler((e) => {
        const dir = navDirection(e);
        if (!dir) return true;
        if (e.type === 'keydown') latest.current.onNavigate(dir);
        e.preventDefault();
        return false;
      });
      const input = xterm.onData((data) => link.sendInput(data));
      const resize = xterm.onResize((size) => link.sendResize(size));
      xterm.textarea?.addEventListener('focus', () => latest.current.onFocus());
      const observer = new ResizeObserver(() => fitAddon.fit());
      observer.observe(el);
      fitAddon.fit();
      if (latest.current.focused) xterm.focus();
      return () => {
        observer.disconnect();
        input.dispose();
        resize.dispose();
        link.close();
        xterm.dispose();
        term.current = null;
        fit.current = null;
      };
    }
  }, [props.src]);

  useEffect(() => {
    if (term.current) term.current.options.theme = theme;
  }, [theme]);

  // 글꼴이 바뀌면 다시 그리고 열·행 수를 다시 잰다. 바뀐 열·행은 `onResize`가 PTY에 보낸다.
  useEffect(() => {
    let disposed = false;
    loadFont(font.family).then(() => {
      const xterm = term.current;
      if (disposed || !xterm) return;
      xterm.options.fontFamily = font.family;
      xterm.options.fontSize = font.size;
      fit.current?.fit();
    });
    return () => {
      disposed = true;
    };
  }, [font.family, font.size]);

  useEffect(() => {
    if (props.focused) term.current?.focus();
    else term.current?.blur();
  }, [props.focused]);

  // 바깥 칸이 여백과 바탕을 갖는다. FitAddon은 안쪽 칸의 크기로 열 수를 잰다.
  return (
    <div className="xterm-box" style={{ background: theme.background }}>
      <div ref={box} />
    </div>
  );
}
