import { createContext, useContext, useEffect, useRef, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import { Terminal as XTerm, type ITheme } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { attachPty, openFile, savePastedImage } from './api';
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

/** 다른 수식 키 없는 Ctrl+글쇠. 한글 입력 중에도 맞도록 글자(`key`)가 아니라 자리(`code`)를 본다. */
function plainCtrl(e: KeyboardEvent, code: string): boolean {
  return e.ctrlKey && !e.shiftKey && !e.altKey && !e.metaKey && e.code === code;
}
export type XtermFontValue = { family: string; size: number };
/**
 * 기본 글꼴 Jetendard(번들, styles.css)는 한글이 꼭 두 칸이라 글자 사이에 틈이 없다. 기본 글꼴로 카드가 80열이다(`.card`의 폭).
 * 근거: docs/design.md 「설정」.
 */
export const DEFAULT_TERMINAL_FONT: XtermFontValue = { family: '"Jetendard", monospace', size: 12 };
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

/**
 * 카드 머리를 누르면 브라우저가 누른 뒤에 카드에 포커스를 준다. 그 뒤에 터미널로 옮기려고 다음 프레임에 준다.
 * 입력창(예: 이름 바꾸기)에 포커스가 있으면 옮기지 않는다. 터미널 연결은 늦게 끝나므로 사람이 쓰는 글을 끊는다.
 */
/**
 * 터미널 링크를 연다. `file://`은 데몬이 그 기계의 기본 앱으로 연다(브라우저 페이지는 열지 못한다). `http(s)`는 새 탭이다. 다른 형식은 열지 않는다.
 * Windows 경로(`file:///C:/…`)는 앞의 `/`를 뗀다.
 */
function openLink(uri: string) {
  const url = URL.parse(uri);
  if (url?.protocol === 'file:') {
    const path = decodeURIComponent(url.pathname);
    openFile(/^\/[A-Za-z]:/.test(path) ? path.slice(1) : path).catch((err: Error) => console.error(err));
  } else if (url?.protocol === 'http:' || url?.protocol === 'https:') {
    window.open(url.href, '_blank', 'noopener');
  }
}

function focusSoon(xterm: XTerm) {
  requestAnimationFrame(() => {
    if (!(document.activeElement instanceof HTMLInputElement)) xterm.focus();
  });
}

/** 지금 모드의 터미널 테마. `App`이 설정과 모드로 정한다. */
export const XtermTheme = createContext<ITheme>(TERMINAL_THEMES[DEFAULT_TERMINAL_THEME.dark]);
/** 모든 터미널의 글꼴. `App`이 설정으로 정한다. */
export const XtermFont = createContext<XtermFontValue>(DEFAULT_TERMINAL_FONT);

/**
 * `src`는 PTY WebSocket 경로다(`nodePty`·`loginPty`). `loadRenderer`는 연 뒤에 렌더러 애드온을 붙인다. 없으면 DOM 렌더러로 그린다.
 */
export function Terminal(props: { src: string; focused: boolean; onFocus: () => void; onNavigate: (dir: Direction) => void; loadRenderer?: (xterm: XTerm) => void }) {
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
        // 에이전트가 적은 링크(OSC 8)를 누르면 연다. 기본값은 http(s)가 아닌 링크를 무시하고, 브라우저는 file://을 열지 못한다.
        linkHandler: { allowNonHttpProtocols: true, activate: (_event, uri) => openLink(uri) },
      });
      const fitAddon = new FitAddon();
      xterm.loadAddon(fitAddon);
      xterm.open(el);
      latest.current.loadRenderer?.(xterm);
      term.current = xterm;
      fit.current = fitAddon;
      const link = attachPty(props.src, {
        onAttach: () => {
          // 다시 붙으면 backlog를 처음부터 받으므로 화면을 비운다.
          xterm.reset();
          link.sendResize({ cols: xterm.cols, rows: xterm.rows });
        },
        onOutput: (bytes) => xterm.write(bytes),
        onNotice: (notice) => {
          switch (notice) {
            case 'gone':
              xterm.write(`${i18next.t('terminal.gone')}\r\n`);
          }
        },
      });
      // xterm은 Ctrl+C·Ctrl+V를 늘 `^C`·`^V`로 보내고 브라우저 복사·붙여넣기를 막는다.
      xterm.attachCustomKeyEventHandler((e) => {
        const dir = navDirection(e);
        if (dir) {
          if (e.type === 'keydown') latest.current.onNavigate(dir);
          e.preventDefault();
          return false;
        }
        // 선택이 없으면 Ctrl+C는 터미널 앱에 `^C`로 간다. 복사한 뒤 선택을 지워 다음 Ctrl+C가 `^C`가 되게 한다.
        if (plainCtrl(e, 'KeyC') && xterm.hasSelection()) {
          if (e.type === 'keydown') {
            void navigator.clipboard.writeText(xterm.getSelection());
            xterm.clearSelection();
          }
          return false;
        }
        // 기본 동작을 막지 않으므로 브라우저가 paste 이벤트를 내고 xterm이 그 글을 보낸다.
        return !plainCtrl(e, 'KeyV');
      });
      // 글이 없는 그림 붙여넣기는 데몬이 그림을 파일로 남기고, 그 경로를 붙여넣는다. 에이전트 CLI는 그림 경로를 첨부로 바꾼다.
      const pasteImage = (e: ClipboardEvent) => {
        if (!e.clipboardData || e.clipboardData.getData('text/plain')) return;
        const image = [...e.clipboardData.files].find((file) => file.type.startsWith('image/'));
        if (!image) return;
        e.stopPropagation();
        e.preventDefault();
        savePastedImage(image).then(
          (path) => xterm.paste(path),
          (err: Error) => xterm.write(`\r\n${err.message}\r\n`),
        );
      };
      // xterm의 paste 처리기보다 먼저 받는다.
      el.addEventListener('paste', pasteImage, true);
      const input = xterm.onData((data) => link.sendInput(data));
      const resize = xterm.onResize((size) => link.sendResize(size));
      xterm.textarea?.addEventListener('focus', () => latest.current.onFocus());
      const observer = new ResizeObserver(() => fitAddon.fit());
      observer.observe(el);
      fitAddon.fit();
      if (latest.current.focused) focusSoon(xterm);
      return () => {
        observer.disconnect();
        el.removeEventListener('paste', pasteImage, true);
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
    if (props.focused && term.current) focusSoon(term.current);
    else term.current?.blur();
  }, [props.focused]);

  // 바깥 칸이 여백과 바탕을 갖는다. FitAddon은 안쪽 칸의 크기로 열 수를 잰다.
  return (
    <div className="xterm-box" style={{ background: theme.background }}>
      <div ref={box} />
    </div>
  );
}
