import { createContext, useContext, useEffect, useRef, useState, type DragEvent, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import { Terminal as XTerm, type ITheme } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { attachPty, openFile, PASTE_LIMIT_BYTES, savePastedFile } from './api';
import type { NodeId } from './gen/NodeId';
import type { Resize } from './gen/Resize';
import type { TerminalFont } from './gen/TerminalFont';
import i18next from './i18n';
import { commandKey } from './platform';
import { DEFAULT_TERMINAL_THEME, TERMINAL_THEMES } from './terminalThemes';

export type Direction = 'left' | 'right' | 'up' | 'down';

const NAV_KEYS: Record<string, Direction> = {
  ArrowLeft: 'left',
  ArrowRight: 'right',
  ArrowUp: 'up',
  ArrowDown: 'down',
};

/** 명령 키(macOS ⌘, 다른 OS Ctrl)+Shift+방향키는 터미널 앱에 넘기지 않고 옆 카드 고르기에 쓴다(목업 「터미널에서 빠져나오기」). */
export function navDirection(e: KeyboardEvent | ReactKeyboardEvent): Direction | null {
  if (!commandKey(e) || !e.shiftKey || e.altKey) return null;
  return NAV_KEYS[e.key] ?? null;
}

/** 접고 펴는 사이드바. */
export type PaneSide = 'left' | 'right';
const PANE_KEYS: Record<string, PaneSide> = { BracketLeft: 'left', BracketRight: 'right' };

/**
 * 명령 키+Shift+`[`·`]`는 왼쪽·오른쪽 사이드바를 접고 편다(목업 toolbar.html 4번). 방향키처럼 터미널 앱에 넘기지 않는다.
 * 터미널은 `Ctrl+Shift+[`와 `Ctrl+[`(Esc)를 구별하지 못한다. Shift 없는 `Ctrl+[`는 그대로 터미널에 간다. 한글 입력 중에도 맞도록 자리(`code`)를 본다.
 */
export function paneKey(e: KeyboardEvent | ReactKeyboardEvent): PaneSide | null {
  if (!commandKey(e) || !e.shiftKey || e.altKey) return null;
  return PANE_KEYS[e.code] ?? null;
}

/** 명령 키+Shift+`F`는 그래프 찾기를 연다(목업 docs/mockups/search.html ①). 접기 키처럼 터미널 앱에 넘기지 않는다. 자리(`code`)를 본다. */
export function findKey(e: KeyboardEvent | ReactKeyboardEvent): boolean {
  return commandKey(e) && e.shiftKey && !e.altKey && e.code === 'KeyF';
}

/**
 * Shift+Enter는 줄바꿈이다. xterm은 Enter와 같은 `\r`을 보내 에이전트 CLI가 바로 제출한다.
 * 대신 `\n`(Ctrl+J)을 보낸다. Claude Code·Codex·OpenCode가 Ctrl+J를 줄바꿈으로 받는다.
 */
const SHIFT_ENTER_INPUT = '\n';
function isShiftEnter(e: KeyboardEvent | ReactKeyboardEvent): boolean {
  return e.key === 'Enter' && e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey;
}

/** 다른 수식 키 없는 명령 키+글쇠(복사·붙여넣기). 한글 입력 중에도 맞도록 글자(`key`)가 아니라 자리(`code`)를 본다. */
function plainCommand(e: KeyboardEvent, code: string): boolean {
  return commandKey(e) && !e.shiftKey && !e.altKey && e.code === code;
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
function openLink(uri: string, node: NodeId | null) {
  const url = URL.parse(uri);
  if (url?.protocol === 'file:') {
    const path = decodeURIComponent(url.pathname);
    openFile(/^\/[A-Za-z]:/.test(path) ? path.slice(1) : path, node).catch((err: Error) => console.error(err));
  } else if (url?.protocol === 'http:' || url?.protocol === 'https:') {
    window.open(url.href, '_blank', 'noopener');
  }
}

type Point = { clientX: number; clientY: number };
type MouseService = {
  getCoords: (event: Point, element: HTMLElement, ...rest: never[]) => unknown;
  getMouseReportCoords: (event: Point, element: HTMLElement) => unknown;
};

/**
 * 그래프 캔버스는 카드를 CSS `scale`로 줄이거나 키워 그린다. xterm은 마우스 위치에서 이 배율을 빼지 않아
 * 링크·글자 선택·TUI 마우스 클릭이 누른 곳과 다른 칸을 고른다(2026-10-20 사람 보고 "터미널 링크가 화면이 클때만 작동함").
 * xterm 내부 `MouseService`의 두 함수 앞에서 위치를 배율 1로 되돌린다. 두 함수는 `clientX`·`clientY`만 읽는다.
 * 내부가 바뀌어 없으면 고치지 않는다.
 */
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

const MB = 1024 * 1024;

/** 셸처럼 작은따옴표로 묶는다. 빈칸·한글 이름도 경로 하나로 읽힌다. 이름 속 `'`는 `'\''`로 쓴다. */
function quotePath(path: string): string {
  return `'${path.replaceAll("'", "'\\''")}'`;
}

/** 파일을 끄는 중인가. 글·링크를 끄면 아니다. */
const hasFiles = (e: DragEvent) => e.dataTransfer.types.includes('Files');

/**
 * 놓은 파일을 데몬에 올리고 경로를 붙여넣는다(목업 docs/mockups/terminal-drop.html). 엔터는 사람이 누른다.
 * 폴더와 큰 파일은 오류 줄을 터미널 화면에만 쓰고, 나머지 파일은 그대로 붙여넣는다.
 */
async function pasteDropped(xterm: XTerm, data: DataTransfer, node: NodeId | null) {
  const files: File[] = [];
  const errors: string[] = [];
  // `items`는 놓기 처리기가 끝나면 비므로 먼저 모두 읽는다.
  for (const item of [...data.items]) {
    const file = item.kind === 'file' ? item.getAsFile() : null;
    if (!file) continue;
    if (item.webkitGetAsEntry()?.isDirectory) errors.push(i18next.t('terminal.folder', { name: file.name }));
    else if (file.size > PASTE_LIMIT_BYTES)
      errors.push(i18next.t('terminal.tooBig', { name: file.name, size: Math.ceil(file.size / MB), limit: PASTE_LIMIT_BYTES / MB }));
    else files.push(file);
  }
  const saved = await Promise.allSettled(files.map((file) => savePastedFile(file, node)));
  const paths = saved.flatMap((r) => (r.status === 'fulfilled' ? [quotePath(r.value)] : []));
  errors.push(...saved.flatMap((r) => (r.status === 'rejected' ? [(r.reason as Error).message] : [])));
  if (errors.length) xterm.write(`\r\n${errors.join('\r\n')}\r\n`);
  if (paths.length) xterm.paste(`${paths.join(' ')} `);
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

/** `focusAsk`가 바뀌면 고른 터미널이 다시 입력 초점을 잡는다. 이미 고른 노드로 다시 갈 때 쓴다(질문 줄 누르기). */
/** `onSender`는 이 터미널의 PTY에 글을 보내는 함수를 받는다. 닫으면 `null`이다. 폰의 키 막대와 입력 줄이 쓴다(목업 docs/mockups/mobile.html ④). */
type TerminalProps = {
  src: string;
  node: NodeId | null;
  focused: boolean;
  focusAsk?: number;
  onFocus: () => void;
  onNavigate: (dir: Direction) => void;
  loadRenderer?: (xterm: XTerm) => void;
  onSender?: (send: ((data: string) => void) | null) => void;
};

/**
 * PTY 크기를 따르는 터미널. 데몬이 알린 열·줄(`PtyNotice` `size`)로 그리고, 글꼴 크기를 바꿔 칸에 맞춘다(목업 docs/mockups/terminal-size.html 안 A).
 * PTY 크기는 바꾸지 않는다. 심플 보기 오른쪽 칸이 쓴다.
 * `src`는 PTY WebSocket 경로다(`nodePty`·`loginPty`). `loadRenderer`는 연 뒤에 렌더러 애드온을 붙인다. 없으면 DOM 렌더러로 그린다.
 */
export function Terminal(props: TerminalProps) {
  return <XtermView {...props} sizing={FOLLOW} />;
}

/** 칸 크기로 PTY 크기를 정하는 터미널. 표준 카드(카드 크기는 데몬 상태다)와 로그인 창이 쓴다. 그래프 확대·축소는 칸 크기를 바꾸지 않는다. */
export function SizingTerminal(props: TerminalProps) {
  return <XtermView {...props} sizing={SET} />;
}

/** 칸이 바뀔 때 할 일. `decide`는 PTY 크기를 정하고, `fit`은 그린 xterm을 칸에 맞춘다. */
type Sizing = { decide: (fit: FitAddon, send: (size: Resize) => void) => void; fit: (xterm: XTerm, box: HTMLElement) => void };
/** PTY 크기를 두고, 글꼴 크기를 바꿔 칸에 맞춘다. CSS 배율로 늘이면 글자 비트맵이 늘어나 흐리다(2026-10-22 사람 지시 "심플모드일때 터미널이 낮은 해상도로 출력됨"). */
const FOLLOW: Sizing = { decide: () => {}, fit: fontToBox };
/** 칸에 들어가는 열·줄을 PTY에 보낸다. 칸이 곧 PTY 크기라 배율은 거의 1이다. */
const SET: Sizing = {
  decide: (fit, send) => {
    const proposed = fit.proposeDimensions();
    if (proposed && proposed.cols > 0 && proposed.rows > 0) send({ cols: proposed.cols, rows: proposed.rows });
  },
  fit: scaleToBox,
};

/** 글꼴 크기 단위. 칸 폭이 바뀔 때마다 WebGL 글자 아틀라스를 다시 만들지 않도록 0.25px씩 바꾼다. */
const FONT_STEP = 0.25;

/** 글꼴 크기를 칸에 들어가는 가장 큰 값으로 바꾸고 칸 가운데에 둔다. 배율은 쓰지 않는다. */
function fontToBox(xterm: XTerm, box: HTMLElement) {
  const el = xterm.element;
  const scale = boxScale(xterm, box);
  if (scale === null || !el) return;
  const current = xterm.options.fontSize ?? DEFAULT_TERMINAL_FONT.size;
  // 한 단계 키우면 넘칠 만큼만 남았으면 그대로 둔다.
  if (scale < 1 || scale >= 1 + FONT_STEP / current) {
    let size = Math.max(FONT_STEP, Math.floor((current * scale) / FONT_STEP) * FONT_STEP);
    xterm.options.fontSize = size;
    // 글자 칸은 기기 픽셀로 반올림되어 글꼴 크기에 딱 비례하지 않는다. 넘치면 한 단계씩 줄인다.
    while (size > FONT_STEP && (boxScale(xterm, box) ?? 1) < 1) {
      size -= FONT_STEP;
      xterm.options.fontSize = size;
    }
  }
  el.style.transform = `translateX(${(box.clientWidth - el.offsetWidth) / 2}px)`;
}

/** 배율 없이 그린 xterm을 칸에 넣는 배율. 아직 그리지 않았으면 `null`이다. */
function boxScale(xterm: XTerm, box: HTMLElement): number | null {
  const el = xterm.element;
  if (!el) return null;
  el.style.transform = '';
  const width = el.offsetWidth;
  const height = el.offsetHeight;
  if (width === 0 || height === 0 || box.clientWidth === 0 || box.clientHeight === 0) return null;
  return Math.min(box.clientWidth / width, box.clientHeight / height);
}

/** xterm을 칸 가운데에 맞춘다. 칸보다 크면 줄이고 작으면 늘인다. 마우스 위치는 `unscaleMouse`가 배율을 뺀다. */
function scaleToBox(xterm: XTerm, box: HTMLElement) {
  const scale = boxScale(xterm, box);
  const el = xterm.element;
  if (scale === null || !el) return;
  const left = (box.clientWidth - el.offsetWidth * scale) / 2;
  el.style.transform = `translateX(${left}px) scale(${scale})`;
}

function XtermView(props: TerminalProps & { sizing: Sizing }) {
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
  // 칸이나 글꼴이 바뀌면 부른다. 크기를 정하는 터미널은 PTY 크기를 보내고, 모든 터미널은 배율을 다시 잰다.
  const redecide = useRef<() => void>(() => {});
  // 끄는 파일 수. 끄지 않으면 없다.
  const [dragging, setDragging] = useState<number | null>(null);

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
        linkHandler: { allowNonHttpProtocols: true, activate: (_event, uri) => openLink(uri, latest.current.node) },
      });
      const fitAddon = new FitAddon();
      xterm.loadAddon(fitAddon);
      xterm.open(el);
      unscaleMouse(xterm);
      latest.current.loadRenderer?.(xterm);
      term.current = xterm;
      const rescale = () => latest.current.sizing.fit(xterm, el);
      const decide = () => latest.current.sizing.decide(fitAddon, (size) => link.sendResize(size));
      redecide.current = () => {
        decide();
        rescale();
      };
      const link = attachPty(props.src, {
        onAttach: () => {
          // 다시 붙으면 backlog를 처음부터 받으므로 화면을 비운다.
          xterm.reset();
          decide();
        },
        onOutput: (bytes) => xterm.write(bytes),
        onNotice: (notice) => {
          if (notice === 'gone') xterm.write(`${i18next.t('terminal.gone')}\r\n`);
          // backlog보다 먼저 온다. 그 크기로 backlog를 그린다.
          else xterm.resize(notice.size.cols, notice.size.rows);
        },
      });
      // xterm은 Ctrl+C·Ctrl+V를 늘 `^C`·`^V`로 보내고 브라우저 복사·붙여넣기를 막는다. 복사·붙여넣기는 명령 키(macOS ⌘, 다른 OS Ctrl)로 한다.
      xterm.attachCustomKeyEventHandler((e) => {
        const dir = navDirection(e);
        if (dir) {
          if (e.type === 'keydown') latest.current.onNavigate(dir);
          e.preventDefault();
          return false;
        }
        if (isShiftEnter(e) && !e.isComposing) {
          // 한글 조합을 끝내는 Enter는 compositionend 뒤 조합 없는 keydown으로 다시 온다. xterm은 조합 글자를 compositionend의 setTimeout(0)에서 보낸다.
          // 줄바꿈도 setTimeout(0)으로 미뤄 그 뒤에 보낸다. 바로 보내면 "있"+Shift+Enter가 `\n있`이 됐다(2026-10-22 사람 보고).
          if (e.type === 'keydown') setTimeout(() => link.sendInput(SHIFT_ENTER_INPUT));
          e.preventDefault();
          return false;
        }
        // 접기·찾기 키는 막지 않고 창으로 올려 보낸다. 창의 키 처리(App)가 사이드바를 바꾸고 찾기 칸으로 간다.
        if (paneKey(e) || findKey(e)) return false;
        // 선택이 없으면 명령 키+C는 터미널 앱에 간다(macOS는 Ctrl+C가 늘 `^C`다). 복사한 뒤 선택을 지워 다음 Ctrl+C가 `^C`가 되게 한다.
        if (plainCommand(e, 'KeyC') && xterm.hasSelection()) {
          if (e.type === 'keydown') {
            void navigator.clipboard.writeText(xterm.getSelection());
            xterm.clearSelection();
          }
          return false;
        }
        // 기본 동작을 막지 않으므로 브라우저가 paste 이벤트를 내고 xterm이 그 글을 보낸다.
        return !plainCommand(e, 'KeyV');
      });
      // 글이 없는 그림 붙여넣기는 데몬이 그림을 파일로 남기고, 그 경로를 붙여넣는다. 에이전트 CLI는 그림 경로를 첨부로 바꾼다.
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
      // xterm의 paste 처리기보다 먼저 받는다.
      el.addEventListener('paste', pasteImage, true);
      const input = xterm.onData((data) => link.sendInput(data));
      latest.current.onSender?.((data) => link.sendInput(data));
      // 크기가 바뀐 뒤 그린 칸 크기로 배율을 다시 잰다.
      const resize = xterm.onResize(() => requestAnimationFrame(rescale));
      xterm.textarea?.addEventListener('focus', () => latest.current.onFocus());
      const observer = new ResizeObserver(() => redecide.current());
      observer.observe(el);
      if (latest.current.focused) focusSoon(xterm);
      return () => {
        observer.disconnect();
        el.removeEventListener('paste', pasteImage, true);
        input.dispose();
        resize.dispose();
        latest.current.onSender?.(null);
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

  // 글꼴이 바뀌면 다시 그리고, 크기를 정하는 터미널은 열·행 수를 다시 잰다. 글자 칸 크기가 바뀌므로 배율도 다시 잰다.
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

  // 바깥 칸이 여백과 바탕을 갖는다. FitAddon은 안쪽 칸의 크기로 열 수를 재고, 배율도 안쪽 칸에 맞춘다.
  return (
    <div className="xterm-box" style={{ background: theme.background }} onDragOver={onDragOver} onDragLeave={onDragLeave} onDrop={onDrop}>
      <div ref={box} />
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
