import { createContext, useContext, useEffect, useRef, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import { Terminal as XTerm, type ITheme } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { attachPty } from './api';
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

const FONT_SIZE = 12;

/** 지금 모드의 터미널 테마. `App`이 설정과 모드로 정한다. */
export const XtermTheme = createContext<ITheme>(TERMINAL_THEMES[DEFAULT_TERMINAL_THEME.dark]);

/** `src`는 PTY WebSocket 경로다(`nodePty`·`loginPty`). */
export function Terminal(props: { src: string; focused: boolean; onFocus: () => void; onNavigate: (dir: Direction) => void }) {
  const box = useRef<HTMLDivElement>(null);
  const term = useRef<XTerm | null>(null);
  const latest = useRef(props);
  latest.current = props;
  const theme = useContext(XtermTheme);
  const latestTheme = useRef(theme);
  latestTheme.current = theme;

  useEffect(() => {
    let disposed = false;
    let cleanup = () => {};
    // 글꼴을 읽기 전에 열면 글자 폭을 잘못 재고 열 수가 틀린다.
    document.fonts.load(`${FONT_SIZE}px "JetBrains Mono"`).then(() => {
      if (disposed || !box.current) return;
      cleanup = mount(box.current);
    });
    return () => {
      disposed = true;
      cleanup();
    };

    function mount(el: HTMLDivElement) {
      const xterm = new XTerm({
        fontFamily: '"JetBrains Mono", monospace',
        fontSize: FONT_SIZE,
        lineHeight: 1.2,
        theme: latestTheme.current,
      });
      const fit = new FitAddon();
      xterm.loadAddon(fit);
      xterm.open(el);
      term.current = xterm;
      const link = attachPty(props.src, {
        onAttach: () => {
          // 다시 붙으면 backlog를 처음부터 받으므로 화면을 비운다.
          xterm.reset();
          link.sendResize({ cols: xterm.cols, rows: xterm.rows });
          if (latest.current.focused) xterm.focus();
        },
        onOutput: (bytes) => xterm.write(bytes),
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
      const observer = new ResizeObserver(() => fit.fit());
      observer.observe(el);
      fit.fit();
      if (latest.current.focused) xterm.focus();
      return () => {
        observer.disconnect();
        input.dispose();
        resize.dispose();
        link.close();
        xterm.dispose();
        term.current = null;
      };
    }
  }, [props.src]);

  useEffect(() => {
    if (term.current) term.current.options.theme = theme;
  }, [theme]);

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
