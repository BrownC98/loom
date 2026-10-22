import { createContext, useContext, useEffect, useRef, useState, type DragEvent, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import { Terminal as XTerm, type ITheme } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { attachPty, openFile, PASTE_LIMIT_BYTES, savePastedFile } from './api';
import type { NodeId } from './gen/NodeId';
import type { PtyRequest } from './gen/PtyRequest';
import type { Resize } from './gen/Resize';
import type { TerminalFont } from './gen/TerminalFont';
import i18next from './i18n';
import { commandKey } from './platform';
import { DEFAULT_TERMINAL_THEME, TERMINAL_THEMES } from './terminalThemes';
import { attachTouchGestures, type TouchSelection } from './touchTerminal';

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
 * 터미널과 받은편지함 글의 링크를 연다. `file://`은 데몬이 그 기계의 기본 앱으로 연다(브라우저 페이지는 열지 못한다). `http(s)`는 새 탭이다. 다른 형식은 열지 않는다.
 * Windows 경로(`file:///C:/…`)는 앞의 `/`를 뗀다.
 */
export function openLink(uri: string, node: NodeId | null) {
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

/** 화면이 PTY 크기를 쥐는 방법(docs/design.md 「데몬 하나」 PTY 열·줄). `whenHidden`은 화면이 가려질 때 보낸다. */
export type SizeHold = { claim: (size: Resize) => PtyRequest; whenHidden: PtyRequest | null };
/** PC 화면: 쥔 크기는 다른 화면이 쥘 때까지 남는다. */
export const keepSize: SizeHold = { claim: (size) => ({ resize: size }), whenHidden: null };
/** 폰: 보는 동안만 빌린다. 가려지거나 연결이 닫히면 데몬이 빌리기 전 크기로 돌린다. */
export const borrowSize: SizeHold = { claim: (size) => ({ borrow: size }), whenHidden: 'giveBack' };

/** `focusAsk`가 바뀌면 고른 터미널이 다시 입력 초점을 잡는다. 이미 고른 노드로 다시 갈 때 쓴다(질문 줄 누르기). */
/** `onSender`는 이 터미널의 PTY에 글을 보내는 함수를 받는다. 닫으면 `null`이다. 폰의 키 막대와 입력 줄이 쓴다(목업 docs/mockups/mobile.html ④). */
type TerminalProps = {
  src: string;
  node: NodeId | null;
  focused: boolean;
  focusAsk?: number;
  onFocus: () => void;
  onNavigate: (dir: Direction) => void;
  onSender?: (send: ((data: string) => void) | null) => void;
  /** 붙여넣을 글을 받는 함수. xterm이 붙여넣기 모드(bracketed paste)를 알아서 감싼다. 닫으면 `null`이다. */
  onPaster?: (paste: ((text: string) => void) | null) => void;
  /** 출력을 그리는 빠르기. 없으면 `live`다. */
  draw?: Draw;
  /** 없으면 `keepSize`다. */
  hold?: SizeHold;
};

/**
 * `live`: 출력이 올 때마다 그린다(xterm이 한 프레임에 한 번 그린다). `slow`: 출력을 모아 `SLOW_DRAW_MS`마다 한 번에 그린다.
 * `hidden`: 화면 밖이다. 출력을 모아 두고 `live`·`slow`가 되면 그린다. 화면 밖 터미널도 그리면 문서 레이아웃이 매번 났다.
 * 모은 출력은 버리지 않는다.
 * 터미널마다 매 프레임 줄을 다시 그리는 것이 그래프의 가장 큰 비용이었다(실측: 출력하는 카드 10개, CPU 4배 감속에서 메인 스레드 포화).
 */
export type Draw = 'live' | 'slow' | 'hidden';
/** 초당 4번. 2026-10-22 사람 결정 Q431 A "약 4fps". */
const SLOW_DRAW_MS = 250;
/** `slow` 터미널마다 모은 출력을 그리는 함수. 한 타이머가 모두 부른다. 터미널마다 타이머를 두면 그리는 프레임이 흩어져 프레임마다 레이아웃이 났다. */
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

/**
 * xterm을 칸 가운데에 두는 transform. 아직 그리지 않았으면 `null`이다. 다른 화면이 정한 PTY 크기가 칸보다 크면 줄인다. 늘이지는 않는다: 글자 그림이 늘어나 흐려진다(2026-10-22 사람 지시 "심플모드일때 터미널이 낮은 해상도로 출력됨", "배율확대 하지말고 폰트크기 원대로 유지해").
 * 레이아웃을 읽기만 한다. `offsetWidth`는 transform을 빼고 잰다. 그래서 여러 터미널을 다 읽은 뒤 쓰면 강제 레이아웃이 한 번이다(실측: 카드 10개 확대·축소에서 터미널마다 두 번이었다).
 * 마우스 위치는 `unscaleMouse`가 배율을 뺀다.
 */
function boxTransform(xterm: XTerm, box: HTMLElement): string | null {
  const el = xterm.element;
  if (!el) return null;
  const width = el.offsetWidth;
  const height = el.offsetHeight;
  if (width === 0 || height === 0 || box.clientWidth === 0 || box.clientHeight === 0) return null;
  const fit = Math.min(box.clientWidth / width, box.clientHeight / height);
  if (fit < 1) return `translateX(${(box.clientWidth - width * fit) / 2}px) scale(${fit})`;
  // 글자 캔버스가 기기 픽셀 경계에 서야 선명하다. 가운데 맞춤이나 위 칸 높이가 0.5px 같은 소수 위치에 두면 흐렸다(2026-10-22 사람 지시 "특정 터미널 크기에서 글자가 흐린 문제").
  // 그래프 안 카드는 확대 배율 `zoom`만큼 커져 보인다. 화면 픽셀로 맞춘 뒤 칸 좌표로 되돌린다.
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

/** 그린 터미널: 칸 → xterm. 칸 크기는 그대로인데 화면 위치만 바뀔 때 다시 맞춘다. */
const shown = new Map<HTMLElement, XTerm>();

/** 그래프를 옮기거나 카드를 끈 뒤 부른다. 위치가 기기 픽셀 경계를 벗어나 글자가 흐려진다(2026-10-22 사람 지시 "그래프를 옮길 때도 위치를 다시 맞추도록 … 조치해"). */
export function realignTerminals() {
  const moves = [...shown].map(([box, xterm]) => [xterm.element, boxTransform(xterm, box)] as const);
  for (const [el, transform] of moves) if (el && transform !== null) el.style.transform = transform;
}

/**
 * 칸 크기로 PTY 열·줄을 정하는 터미널. 글꼴 크기는 설정 값 그대로다. 칸이 바뀌면 열·줄이 바뀐다(2026-10-22 사람 지시 "일반적인 터미널의 크기를 조정해도 터미널 폰트 크기는 안변하잖아, 사이드탭 터미널도 동일하게 작동하게 만들어").
 * PTY 크기는 마지막에 칸을 바꾸거나 사람이 쓴 화면을 따른다. 다른 화면은 그 크기를 배율로 맞춰 보인다(`scaleToBox`). 그래프 확대·축소는 칸 크기를 바꾸지 않는다.
 * `src`는 PTY WebSocket 경로다(`nodePty`·`loginPty`).
 */
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
  // 끄는 파일 수. 끄지 않으면 없다.
  const [dragging, setDragging] = useState<number | null>(null);
  // 손가락으로 고른 글이 있으면 그 자리. 복사 버튼이 뜬다.
  const [picked, setPicked] = useState<TouchSelection | null>(null);

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
      term.current = xterm;
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
      // 사람이 지금 쓰는 화면이 PTY 크기를 다시 정한다(docs/design.md 「저장」 PTY 열·줄). xterm 크기는 데몬이 알린 PTY 크기다.
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
      // `slow`일 때 모은 출력과 크기 알림. 온 차례대로 xterm에 넘긴다. 크기 알림이 그 뒤 출력의 모양을 정하기 때문이다.
      const held: (() => void)[] = [];
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
      const link = attachPty(props.src, {
        onAttach: () => {
          // 다시 붙으면 backlog를 처음부터 받으므로 화면을 비운다. 모은 출력도 지난 연결의 것이다.
          held.length = 0;
          xterm.reset();
          decide();
        },
        onOutput: (bytes) => feed(() => xterm.write(bytes)),
        onNotice: (notice) => {
          if (notice === 'gone') feed(() => xterm.write(`${i18next.t('terminal.gone')}\r\n`));
          // backlog보다 먼저 온다. 그 크기로 backlog를 그린다.
          else feed(() => xterm.resize(notice.size.cols, notice.size.rows));
        },
      });
      // xterm은 Ctrl+C·Ctrl+V를 늘 `^C`·`^V`로 보내고 브라우저 복사·붙여넣기를 막는다. 복사·붙여넣기는 명령 키(macOS ⌘, 다른 OS Ctrl)로 한다.
      xterm.attachCustomKeyEventHandler((e) => {
        if (e.type === 'keydown') claim();
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
      latest.current.onSender?.((data) => {
        claim();
        link.sendInput(data);
      });
      // 크기가 바뀐 뒤 그린 칸 크기로 배율을 다시 잰다.
      latest.current.onPaster?.((text) => xterm.paste(text));
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
        latest.current.onPaster?.(null);
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
    const xterm = term.current;
    if (!xterm) return;
    void navigator.clipboard.writeText(xterm.getSelection()).catch((err: Error) => console.error(err));
    xterm.clearSelection();
  };

  // 바깥 칸이 여백과 바탕을 갖는다. FitAddon은 안쪽 칸의 크기로 열 수를 재고, 배율도 안쪽 칸에 맞춘다.
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
