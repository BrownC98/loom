// 폰 폭 화면(목업 docs/mockups/mobile.html M35, 2026-10-22 사람 승인 Q279·Q280). 배치만 맡는다.
// 질문 목록·트리·사용량 줄은 App이 데스크톱과 같은 부품으로 만들어 넘긴다. 상태 원천은 데스크톱과 같은 데몬 이벤트다.
import { ArrowDown, ArrowLeft, ArrowRight, ArrowUp, Bell, Check, ChevronLeft, CornerDownLeft, EllipsisVertical, ListTree, Moon, Sun, SunMoon, TriangleAlert, X, type LucideIcon } from 'lucide-react';
import { useEffect, useRef, useState, type FormEvent, type MouseEvent as ReactMouseEvent, type PointerEvent as ReactPointerEvent, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { nodePty, relaunchNode, relaunchQueued, removeNode } from './api';
import type { Theme } from './gen/Theme';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import { bookmarkToggle } from './BookmarkButton';
import { QuestionIcon } from './Dock';
import { InboxPanel, oldestWait, PillHead, waitCount } from './Inbox';
import { asking, stateClass, stateLabel } from './NodeCard';
import { linkParts, plainText } from './links';
import { borrowSize, openLink, SizingTerminal } from './Terminal';
import { LONG_PRESS_MS, readClipboard, SLOP_PX } from './touchTerminal';

/** 터치 화면이고 이 폭 아래일 때 폰 배치를 쓴다(목업 ①). 마우스 PC의 좁은 창(설치한 웹 앱 창 등)은 데스크톱 배치다(2026-10-22 사람 지적 "지금 윈도우 버전 실행하니까 왜 앱 화면이 나오냐"). */
const PHONE_QUERY = '(max-width: 639px) and (pointer: coarse)';

export function usePhone(): boolean {
  const [phone, setPhone] = useState(() => matchMedia(PHONE_QUERY).matches);
  useEffect(() => {
    const media = matchMedia(PHONE_QUERY);
    const change = () => setPhone(media.matches);
    media.addEventListener('change', change);
    return () => media.removeEventListener('change', change);
  }, []);
  return phone;
}

/** 화면마다 주소가 다르다(목업 ①). 폰의 뒤로 몸짓과 알림 링크가 맞는 화면을 연다. */
export type PhoneRoute = { screen: 'inbox' } | { screen: 'nodes' } | { screen: 'node'; id: NodeId };

function readRoute(): PhoneRoute {
  const hash = location.hash;
  if (hash === '#/inbox') return { screen: 'inbox' };
  const node = /^#\/node\/(\d+)$/.exec(hash);
  return node ? { screen: 'node', id: Number(node[1]) as NodeId } : { screen: 'nodes' };
}

function routeHash(route: PhoneRoute): string {
  return route.screen === 'node' ? `#/node/${route.id}` : `#/${route.screen}`;
}

/** 탭을 바꿀 때는 지금 주소를 바꾸고, 노드를 열 때는 주소를 쌓는다. 그래서 뒤로 몸짓이 앞 탭으로 돌아간다. */
export function usePhoneRoute(): [PhoneRoute, (next: PhoneRoute) => void] {
  const [route, setRoute] = useState(readRoute);
  useEffect(() => {
    const onHash = () => setRoute(readRoute());
    window.addEventListener('hashchange', onHash);
    return () => window.removeEventListener('hashchange', onHash);
  }, []);
  const go = (next: PhoneRoute) => {
    // 열린 판의 기록 칸 위에 쌓지 않는다. 판이 닫혀도 칸을 지울 수 없어, 뒤로 몸짓이 한 번 헛돈다.
    if (next.screen === 'node' && !isSheetEntry(history.state)) history.pushState(null, '', routeHash(next));
    else history.replaceState(null, '', routeHash(next));
    setRoute(next);
  };
  return [route, go];
}

export function Phone(props: {
  route: PhoneRoute;
  /** 지금 그리는 테마(이 폰에서 고른 것, 없으면 PC 설정). */
  theme: Theme;
  onTheme: (next: Theme) => void;
  onRoute: (next: PhoneRoute) => void;
  waiting: NodeView[];
  nodes: ReadonlyMap<NodeId, NodeView>;
  agentLabel: (view: NodeView) => string;
  /** 에이전트가 이어 띄우기를 받으면 노드 메뉴에 [다시 띄우기]를 둔다(데스크톱 `NodeMenu`와 같다). */
  resumable: (view: NodeView) => boolean;
  online: boolean;
  /** 질문 탭 몸통: 데스크톱 「질문」 탭과 같은 목록. */
  inbox: ReactNode;
  /** 노드 탭 몸통: 찾기 칸과 사이드바 트리. */
  tree: ReactNode;
  /** 아래 탭 위의 줄(사용량). 없으면 줄이 없다. */
  foot?: ReactNode;
}) {
  const { t } = useTranslation();
  const { route } = props;
  const count = waitCount(props.waiting);
  const view = route.screen === 'node' ? props.nodes.get(route.id) : undefined;
  if (route.screen === 'node')
    return (
      <div className="phone">
        {view ? (
          <NodeScreen key={view.id} view={view} waiting={props.waiting} agentLabel={props.agentLabel(view)} resumable={props.resumable(view)} online={props.online} onBack={() => history.back()} onOpen={(id) => props.onRoute({ screen: 'node', id })} />
        ) : (
          <>
            <header className="phone-top">
              <button type="button" className="phone-icon" aria-label={t('phone.back')} onClick={() => props.onRoute({ screen: 'nodes' })}>
                <ChevronLeft aria-hidden="true" />
              </button>
            </header>
            <p className="phone-empty">{t('phone.noNode')}</p>
          </>
        )}
      </div>
    );
  const inbox = route.screen === 'inbox';
  return (
    <div className="phone">
      <header className="phone-top">
        <span className="phone-title">
          <b>{inbox ? t('phone.inbox') : t('phone.nodes')}</b>
        </span>
        <ThemeButton theme={props.theme} onTheme={props.onTheme} />
      </header>
      {!props.online && <Offline />}
      <main className="phone-body" aria-label={inbox ? t('phone.inbox') : t('phone.nodes')}>
        {inbox && count === 0 ? (
          <p className="phone-empty center">{t('phone.noQuestions')}</p>
        ) : inbox ? (
          props.inbox
        ) : (
          props.tree
        )}
      </main>
      {props.foot}
      <nav className="phone-tabs" aria-label={t('phone.tabs')}>
        <button type="button" aria-current={inbox ? undefined : 'page'} onClick={() => props.onRoute({ screen: 'nodes' })}>
          <ListTree aria-hidden="true" />
          {t('phone.nodes')}
        </button>
        <button type="button" aria-current={inbox ? 'page' : undefined} onClick={() => props.onRoute({ screen: 'inbox' })}>
          <Bell aria-hidden="true" />
          {t('phone.inbox')}
          {count > 0 && <span className="count">{count}</span>}
        </button>
      </nav>
    </div>
  );
}

const THEME_CHOICES = [
  { value: 'system', label: 'phone.themeSystem', icon: SunMoon },
  { value: 'light', label: 'theme.light', icon: Sun },
  { value: 'dark', label: 'theme.dark', icon: Moon },
] as const satisfies readonly { value: Theme; label: string; icon: LucideIcon }[];

/** 머리 줄 오른쪽 단추. 지금 고른 테마의 아이콘을 보이고, 누르면 세 줄 판을 연다(목업 app-theme.html M7). */
function ThemeButton(props: { theme: Theme; onTheme: (next: Theme) => void }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const Current = THEME_CHOICES.find((choice) => choice.value === props.theme)?.icon ?? SunMoon;
  return (
    <>
      <button type="button" className="phone-icon" aria-label={t('phone.theme')} onClick={() => setOpen(true)}>
        <Current aria-hidden="true" />
      </button>
      {open && (
        <MenuSheet
          title={t('phone.theme')}
          onClose={() => setOpen(false)}
          items={THEME_CHOICES.map((choice) => ({ label: t(choice.label), icon: <choice.icon />, checked: choice.value === props.theme, run: () => props.onTheme(choice.value) }))}
        />
      )}
    </>
  );
}

function Offline() {
  const { t } = useTranslation();
  return (
    <p className="phone-offline" role="status">
      <TriangleAlert aria-hidden="true" />
      {t('phone.offline')}
    </p>
  );
}

/** 키 막대가 보내는 바이트. 화살표는 일반 커서 모드(CSI)다. Ctrl을 켜면 xterm과 같은 `CSI 1;5`를 쓴다. */
const ARROWS = [
  { key: 'D', label: 'left', icon: ArrowLeft },
  { key: 'A', label: 'up', icon: ArrowUp },
  { key: 'B', label: 'down', icon: ArrowDown },
  { key: 'C', label: 'right', icon: ArrowRight },
] as const;
const ESC = '\x1b';
const CTRL_C = '\x03';

/** Ctrl+글자. `@`·`A`~`Z`·`[`·`\`·`]`·`^`·`_`(소문자 포함)만 제어 문자가 있다. 다른 글자는 그대로다. */
function controlOf(char: string): string {
  const code = char.toUpperCase().charCodeAt(0);
  return code >= 0x40 && code <= 0x5f ? String.fromCharCode(code - 0x40) : char;
}

/** 노드 하나의 전체 화면 터미널(목업 ④⑤). */
function NodeScreen(props: { view: NodeView; /** 모든 노드의 대기(`waitingNodes`). 알약이 보인다. */ waiting: NodeView[]; agentLabel: string; resumable: boolean; online: boolean; onBack: () => void; onOpen: (id: NodeId) => void }) {
  const { t } = useTranslation();
  const { view } = props;
  const send = useRef<((data: string) => void) | null>(null);
  const paster = useRef<((text: string) => void) | null>(null);
  const [pasteFailed, setPasteFailed] = useState(false);
  const [ctrl, setCtrl] = useState(false);
  const [text, setText] = useState('');
  const [sheet, setSheet] = useState<'actions' | 'answer' | 'inbox' | null>(null);
  const [askOpen, setAskOpen] = useState(false);
  const worktree = view.place.worktree.split(/[\\/]/).filter(Boolean).pop() ?? '';
  const asks = asking(view);
  const first = asks[0];
  const pillNode = props.waiting[0];

  const key = (data: string) => {
    send.current?.(data);
    setCtrl(false);
  };
  const paste = () => {
    readClipboard().then(
      (text) => {
        setPasteFailed(false);
        if (text) paster.current?.(text);
      },
      () => setPasteFailed(true),
    );
  };
  const arrow = (code: string) => key(ctrl ? `${ESC}[1;5${code}` : `${ESC}[${code}`);
  const submit = (e: FormEvent) => {
    e.preventDefault();
    key(`${text}\r`);
    setText('');
  };
  const type = (next: string) => {
    // Ctrl을 켠 뒤 친 한 글자는 칸에 넣지 않고 제어 문자로 보낸다.
    if (ctrl && next.length === text.length + 1 && next.startsWith(text)) return key(controlOf(next.slice(-1)));
    setText(next);
  };

  return (
    <>
      <header className="phone-top">
        <button type="button" className="phone-icon" aria-label={t('phone.back')} onClick={props.onBack}>
          <ChevronLeft aria-hidden="true" />
        </button>
        <span className="phone-title">
          <b>{view.name}</b>
          <span>
            {props.agentLabel} · {worktree}
          </span>
        </span>
        <span className={`state ${stateClass(view)}`} aria-live="polite" translate="no">
          {asks.length > 0 ? t('wait.label') : stateLabel(view.state)}
        </span>
        <button type="button" className="phone-icon" aria-label={t('phone.actions')} aria-haspopup="dialog" onClick={() => setSheet('actions')}>
          <EllipsisVertical aria-hidden="true" />
        </button>
      </header>
      {!props.online && <Offline />}
      {first && (
        // 띠를 누르면 글 전체를 펼친다. 접힌 띠는 한 줄로 자른다(2026-10-23 사람 "앱 화면 터미널에 알림떳을때 터치하면 펼쳐지도록 해줘").
        <div className={askOpen ? 'phone-ask open' : 'phone-ask'} onClick={() => setAskOpen((open) => !open)}>
          <QuestionIcon aria-hidden="true" />
          <b>{t('wait.question')}</b>
          {asks.length > 1 && <span className="count">{asks.length}</span>}
          <span className="qid">Q{first.id}</span>
          <span className="text" role="button" tabIndex={0} aria-expanded={askOpen}>
            {askOpen
              ? linkParts(first.text).map((part, index) =>
                  'href' in part ? (
                    <button key={index} type="button" className="doc-link" onClick={(e) => (e.stopPropagation(), openLink(part.href, view.id))}>
                      {part.label}
                    </button>
                  ) : (
                    part.text
                  ),
                )
              : plainText(first.text)}
          </span>
          <button
            type="button"
            className="btn sm"
            aria-haspopup="dialog"
            onClick={(e) => {
              e.stopPropagation();
              setSheet('answer');
            }}
          >
            {t('phone.answer')}
          </button>
        </div>
      )}
      <div className="phone-term" translate="no">
        {/* 모든 노드의 질문 알약(목업 R4, Q459 A). 데스크톱 「한 줄」 알약처럼 터미널 위 가운데에 뜬다. 숫자는 이 노드의 질문도 센다(알림 탭 배지와 같다). 위 사각형 띠는 이 노드의 질문만 보인다. */}
        {pillNode && (
          <div className="inbox-pill phone-pill" data-view="line" data-kind={oldestWait(pillNode).kind}>
            <PillHead node={pillNode} wait={oldestWait(pillNode)} count={waitCount(props.waiting)} line open={sheet === 'inbox'} onToggle={() => setSheet('inbox')} />
          </div>
        )}
        <SizingTerminal src={nodePty(view.id)} node={view.id} hold={borrowSize} focused={false} onFocus={() => {}} onNavigate={() => {}} onSender={(s) => (send.current = s)} onPaster={(p) => (paster.current = p)} />
      </div>
      <div className="phone-dock">
        <div className="phone-keys" role="group" aria-label={t('phone.keys')}>
          <button type="button" onClick={() => key(ESC)}>
            Esc
          </button>
          <button type="button" onClick={() => key('\t')}>
            Tab
          </button>
          <button type="button" aria-pressed={ctrl} onClick={() => setCtrl(!ctrl)}>
            Ctrl
          </button>
          <button type="button" className="paste" onClick={paste}>
            {t('phone.paste')}
          </button>
          <button type="button" aria-label={t('phone.ctrlC')} onClick={() => key(CTRL_C)}>
            ^C
          </button>
          {ARROWS.map(({ key: code, label, icon: Icon }) => (
            <button key={code} type="button" aria-label={t(`phone.arrow.${label}`)} onClick={() => arrow(code)}>
              <Icon aria-hidden="true" />
            </button>
          ))}
        </div>
        {pasteFailed && (
          <p className="phone-note" role="alert">
            {t('phone.pasteDenied')}
          </p>
        )}
        <form className="phone-input" onSubmit={submit}>
          <input value={text} onChange={(e) => type(e.target.value)} aria-label={t('phone.input')} placeholder={t('phone.input')} autoComplete="off" autoCapitalize="off" spellCheck={false} enterKeyHint="send" />
          <button type="submit" className="send" aria-label={t('phone.send')}>
            <CornerDownLeft aria-hidden="true" />
          </button>
        </form>
      </div>
      {sheet === 'actions' && <ActionSheet view={view} resumable={props.resumable} onCtrlC={() => key(CTRL_C)} onClose={() => setSheet(null)} onRemoved={props.onBack} />}
      {sheet === 'inbox' && (
        <Sheet label={t('phone.inbox')} onClose={() => setSheet(null)}>
          {/* 알림 탭과 같은 목록이다. 그 자리에서 답하고, 노드 이름을 누르면 그 노드 화면을 쌓아 연다(뒤로 몸짓은 이 화면으로 온다). */}
          <InboxPanel
            waiting={props.waiting}
            onSelect={(id) => {
              setSheet(null);
              if (id !== view.id) props.onOpen(id);
            }}
            onFocus={(id) => {
              setSheet(null);
              if (id !== view.id) props.onOpen(id);
            }}
          />
        </Sheet>
      )}
      {sheet === 'answer' && (
        <Sheet label={t('phone.answer')} onClose={() => setSheet(null)}>
          <InboxPanel waiting={[view]} onSelect={() => setSheet(null)} onFocus={() => setSheet(null)} />
        </Sheet>
      )}
    </>
  );
}

/** 손가락을 이만큼(px) 끌어 내리고 떼면 판이 닫힌다. */
const DRAG_CLOSE_PX = 80;

/** 판이 열릴 때 쌓는 기록 칸의 표시. 값은 판이 겹친 깊이다. */
type SheetEntry = { loomSheet: number };

function isSheetEntry(state: unknown): state is SheetEntry {
  return typeof state === 'object' && state !== null && typeof (state as SheetEntry).loomSheet === 'number';
}

/** 지금 열린 판 수. 안드로이드 뒤로 몸짓은 WebView `goBack`이라 판마다 기록 칸을 하나 쌓아 그 몸짓이 판만 닫게 한다(2026-10-23 사람 "바텀 시트 다시 닫을 수 있는 UI UX 전부 추가해"). */
let openSheets = 0;

/** 판이 떠 있는 동안 기록 칸 하나를 쌓는다. 뒤로 가면 이 판을 닫고, 다른 길로 닫히면 쌓은 칸을 걷는다. 해시는 바꾸지 않아 라우터는 모른다. */
function useSheetHistory(onClose: () => void) {
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    const depth = ++openSheets;
    history.pushState({ loomSheet: depth } satisfies SheetEntry, '');
    const onPop = () => {
      const now = isSheetEntry(history.state) ? history.state.loomSheet : 0;
      if (now < depth) close.current();
    };
    window.addEventListener('popstate', onPop);
    return () => {
      window.removeEventListener('popstate', onPop);
      openSheets--;
      if (isSheetEntry(history.state) && history.state.loomSheet === depth) history.back();
    };
  }, []);
}

/**
 * 아래에서 올라오는 판. 머리 줄(손잡이·제목·[X])은 늘 보이고 몸통만 스크롤한다.
 * 바탕을 누르거나 Esc나 [X]로 닫고, 머리 줄을 아래로 끌어도 닫힌다(2026-10-23 사람 "앱 알림 알약 리스트 스크롤 안되고 바텀 시트를 다시 닫을 ui/ux 개선하라").
 * 폰 노드 탭의 줄 메뉴와 줄 팝오버도 쓴다.
 */
export function Sheet(props: { label: string; onClose: () => void; children: ReactNode }) {
  const { t } = useTranslation();
  const sheet = useRef<HTMLDivElement>(null);
  const dragFrom = useRef<number | null>(null);
  useSheetHistory(props.onClose);
  useEffect(() => {
    const close = (e: KeyboardEvent) => e.key === 'Escape' && props.onClose();
    document.addEventListener('keydown', close);
    return () => document.removeEventListener('keydown', close);
  }, [props.onClose]);
  const drag = (e: ReactPointerEvent) => {
    if (dragFrom.current === null || !sheet.current) return;
    sheet.current.style.transform = `translateY(${Math.max(0, e.clientY - dragFrom.current)}px)`;
  };
  const drop = (e: ReactPointerEvent) => {
    if (dragFrom.current === null || !sheet.current) return;
    const dy = e.clientY - dragFrom.current;
    dragFrom.current = null;
    sheet.current.style.transform = '';
    if (dy > DRAG_CLOSE_PX) props.onClose();
  };
  return (
    <div className="phone-scrim" onClick={(e) => e.target === e.currentTarget && props.onClose()}>
      <div ref={sheet} className="phone-sheet" role="dialog" aria-modal="true" aria-label={props.label}>
        <div
          className="sheet-head"
          onPointerDown={(e) => {
            if ((e.target as HTMLElement).closest('button')) return;
            dragFrom.current = e.clientY;
            e.currentTarget.setPointerCapture(e.pointerId);
          }}
          onPointerMove={drag}
          onPointerUp={drop}
          onPointerCancel={drop}
        >
          <span className="grip" aria-hidden="true" />
          <h3>{props.label}</h3>
          <button type="button" className="sheet-close" aria-label={t('phone.closeSheet')} onClick={props.onClose}>
            <X aria-hidden="true" />
          </button>
        </div>
        <div className="sheet-body">{props.children}</div>
      </div>
    </div>
  );
}

/** 판의 단추 하나. `confirm`이면 판 안에서 한 번 더 눌러야 한다(확인 창을 따로 띄우지 않는다). `run`이 거절하면 판 아래에 그 글을 보이고 판을 열어 둔다. */
export type MenuItem = { label: string; icon?: ReactNode; run: () => unknown; checked?: boolean; danger?: boolean; confirm?: boolean; disabled?: boolean };

/** 단추 목록 판(목업 폰 노드 목록 R3 ③). 노드 화면의 [⋮]와 노드 탭 줄의 [⋮]·길게 누르기가 연다. */
export function MenuSheet(props: { title: string; items: MenuItem[]; onClose: () => void }) {
  const { t } = useTranslation();
  const [armed, setArmed] = useState<string | null>(null);
  const [refusal, setRefusal] = useState<string | null>(null);
  const press = (item: MenuItem) => {
    if (item.confirm && armed !== item.label) return setArmed(item.label);
    setRefusal(null);
    Promise.resolve(item.run()).then(props.onClose, (err: Error) => setRefusal(err.message));
  };
  return (
    <Sheet label={props.title} onClose={props.onClose}>
      {props.items.map((item) => (
        <button key={item.label} type="button" className={item.danger ? 'btn danger' : 'btn'} disabled={item.disabled} onClick={() => press(item)}>
          {item.icon}
          {armed === item.label ? t('phone.removeConfirm') : item.label}
          {item.checked && <Check className="mark" aria-hidden="true" />}
        </button>
      ))}
      <button type="button" className="btn ghost" onClick={props.onClose}>
        {t('phone.cancel')}
      </button>
      {refusal && (
        <p className="refusal" role="alert">
          {refusal}
        </p>
      )}
    </Sheet>
  );
}

/** 길게 누르기(`LONG_PRESS_MS`, 터미널 낱말 고르기와 같은 시간)로 `onLong`을 부른다. 손가락이 `SLOP_PX` 넘게 움직이면(스크롤) 취소한다. 돌려준 처리기를 줄 단추에 펼쳐 붙인다. */
export function useLongPress(onLong: (row: HTMLElement) => void) {
  const timer = useRef<number | undefined>(undefined);
  const start = useRef<{ x: number; y: number } | null>(null);
  const cancel = () => {
    window.clearTimeout(timer.current);
    start.current = null;
  };
  useEffect(() => cancel, []);
  return {
    onPointerDown: (e: ReactPointerEvent<HTMLElement>) => {
      if (e.pointerType === 'mouse') return;
      start.current = { x: e.clientX, y: e.clientY };
      const row = e.currentTarget;
      timer.current = window.setTimeout(() => {
        start.current = null;
        swallowNextClick();
        onLong(row);
      }, LONG_PRESS_MS);
    },
    onPointerMove: (e: ReactPointerEvent<HTMLElement>) => {
      if (start.current && Math.hypot(e.clientX - start.current.x, e.clientY - start.current.y) > SLOP_PX) cancel();
    },
    onPointerUp: cancel,
    onPointerCancel: cancel,
    // 안드로이드는 길게 누르면 글자 고르기 메뉴를 띄운다.
    onContextMenu: (e: ReactMouseEvent) => e.preventDefault(),
  };
}

/** 손가락을 뗄 때 오는 누르기를 버린다. 그 누르기는 손가락 아래에 막 뜬 판의 바탕에 닿아 판을 닫았다(Pixel 7 시험). 누르기 없이 다음 누름이 오면 버리기를 거둔다. */
function swallowNextClick() {
  const swallow = (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    window.removeEventListener('pointerdown', drop, true);
  };
  const drop = () => window.removeEventListener('click', swallow, true);
  window.addEventListener('click', swallow, { capture: true, once: true });
  window.addEventListener('pointerdown', drop, { capture: true, once: true });
}

/** [⋯] 판(목업 ⑤). */
function ActionSheet(props: { view: NodeView; resumable: boolean; onCtrlC: () => void; onClose: () => void; onRemoved: () => void }) {
  const { t } = useTranslation();
  const bookmark = bookmarkToggle(props.view);
  const resumable = props.resumable && props.view.state.kind === 'running';
  // 열 때 데몬에 물은 값. 답이 오기 전에는 누를 수 없다(데스크톱 `NodeMenu`와 같다).
  const [queued, setQueued] = useState<boolean | null>(null);
  useEffect(() => {
    if (resumable) relaunchQueued(props.view.id).then(setQueued, () => setQueued(false));
  }, [resumable, props.view.id]);
  // 앱에도 터미널 다시 띄우기를 둔다(2026-10-23 사람 "터미널 다시 켜기도 앱에 추가해줘").
  const relaunch: MenuItem[] = resumable ? [{ label: queued ? t('node.relaunchQueued') : t('node.relaunch'), disabled: queued !== false, run: () => relaunchNode(props.view.id) }] : [];
  return (
    <MenuSheet
      title={props.view.name}
      onClose={props.onClose}
      items={[
        { label: t('phone.sendCtrlC'), run: props.onCtrlC },
        ...relaunch,
        // 앱에서도 북마크 영역을 채운다(Q453 A).
        { label: bookmark.label, run: bookmark.run },
        { label: t('phone.remove'), danger: true, confirm: true, run: () => removeNode(props.view.id).then(props.onRemoved) },
      ]}
    />
  );
}
