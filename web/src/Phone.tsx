// 폰 폭 화면(목업 docs/mockups/mobile.html M35, 2026-10-22 사람 승인 Q279·Q280). 배치만 맡는다.
// 질문 목록·트리·사용량 줄은 App이 데스크톱과 같은 부품으로 만들어 넘긴다. 상태 원천은 데스크톱과 같은 데몬 이벤트다.
import { ArrowDown, ArrowLeft, ArrowRight, ArrowUp, Bell, ChevronLeft, CornerDownLeft, EllipsisVertical, ListTree, TriangleAlert } from 'lucide-react';
import { useEffect, useRef, useState, type FormEvent, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { nodePty, removeNode } from './api';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import { QuestionIcon } from './Dock';
import { InboxPanel } from './Inbox';
import { asking, stateClass, stateLabel } from './NodeCard';
import { SizingTerminal } from './Terminal';

/** 이 폭 아래에서 폰 배치를 쓴다(목업 ①). styles.css의 `@media`와 같은 값이다. */
const PHONE_QUERY = '(max-width: 639px)';

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
  if (hash === '#/nodes') return { screen: 'nodes' };
  const node = /^#\/node\/(\d+)$/.exec(hash);
  return node ? { screen: 'node', id: Number(node[1]) as NodeId } : { screen: 'inbox' };
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
    if (next.screen === 'node') history.pushState(null, '', routeHash(next));
    else history.replaceState(null, '', routeHash(next));
    setRoute(next);
  };
  return [route, go];
}

/** 위 막대의 기기 이름. Tailscale 주소(`pc.tail1234.ts.net`)는 앞 이름만 보인다. */
function deviceName(): string {
  const host = location.hostname;
  return /^[\d.]+$|:/.test(host) ? host : host.split('.')[0];
}

export function Phone(props: {
  route: PhoneRoute;
  onRoute: (next: PhoneRoute) => void;
  waiting: NodeView[];
  nodes: ReadonlyMap<NodeId, NodeView>;
  agentLabel: (view: NodeView) => string;
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
  const count = props.waiting.reduce((n, v) => n + v.waits.length, 0);
  const view = route.screen === 'node' ? props.nodes.get(route.id) : undefined;
  if (route.screen === 'node')
    return (
      <div className="phone">
        {view ? (
          <NodeScreen key={view.id} view={view} agentLabel={props.agentLabel(view)} online={props.online} onBack={() => history.back()} onOpen={(id) => props.onRoute({ screen: 'node', id })} />
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
        <span className="phone-mark" aria-hidden="true">
          L
        </span>
        <span className="phone-title">
          <b>{inbox ? t('phone.inbox') : t('phone.nodes')}</b>
          <span>{inbox ? deviceName() : `${deviceName()} · ${t('phone.nodeCount', { count: props.nodes.size })}`}</span>
        </span>
      </header>
      {!props.online && <Offline />}
      <main className="phone-body" aria-label={inbox ? t('phone.inbox') : t('phone.nodes')}>
        {inbox && count === 0 ? (
          <div className="phone-empty">
            <p>{t('phone.noQuestions')}</p>
            <button type="button" className="btn" onClick={() => props.onRoute({ screen: 'nodes' })}>
              {t('phone.toNodes')}
            </button>
          </div>
        ) : inbox ? (
          props.inbox
        ) : (
          props.tree
        )}
      </main>
      {props.foot}
      <nav className="phone-tabs" aria-label={t('phone.tabs')}>
        <button type="button" aria-current={inbox ? 'page' : undefined} onClick={() => props.onRoute({ screen: 'inbox' })}>
          <Bell aria-hidden="true" />
          {t('phone.inbox')}
          {count > 0 && <span className="count">{count}</span>}
        </button>
        <button type="button" aria-current={inbox ? undefined : 'page'} onClick={() => props.onRoute({ screen: 'nodes' })}>
          <ListTree aria-hidden="true" />
          {t('phone.nodes')}
        </button>
      </nav>
    </div>
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
function NodeScreen(props: { view: NodeView; agentLabel: string; online: boolean; onBack: () => void; onOpen: (id: NodeId) => void }) {
  const { t } = useTranslation();
  const { view } = props;
  const send = useRef<((data: string) => void) | null>(null);
  const [ctrl, setCtrl] = useState(false);
  const [text, setText] = useState('');
  const [sheet, setSheet] = useState<'actions' | 'answer' | null>(null);
  const worktree = view.place.worktree.split(/[\\/]/).filter(Boolean).pop() ?? '';
  const asks = asking(view);
  const first = asks[0];

  const key = (data: string) => {
    send.current?.(data);
    setCtrl(false);
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
        <div className="phone-ask">
          <QuestionIcon aria-hidden="true" />
          <b>{t('wait.question')}</b>
          {asks.length > 1 && <span className="count">{asks.length}</span>}
          <span className="qid">Q{first.id}</span>
          <span className="text">{first.text}</span>
          <button type="button" className="btn sm" aria-haspopup="dialog" onClick={() => setSheet('answer')}>
            {t('phone.answer')}
          </button>
        </div>
      )}
      <div className="phone-term" translate="no">
        <SizingTerminal src={nodePty(view.id)} focused={false} onFocus={() => {}} onNavigate={() => {}} onSender={(s) => (send.current = s)} />
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
          <button type="button" aria-label={t('phone.ctrlC')} onClick={() => key(CTRL_C)}>
            ^C
          </button>
          {ARROWS.map(({ key: code, label, icon: Icon }) => (
            <button key={code} type="button" aria-label={t(`phone.arrow.${label}`)} onClick={() => arrow(code)}>
              <Icon aria-hidden="true" />
            </button>
          ))}
        </div>
        <form className="phone-input" onSubmit={submit}>
          <input value={text} onChange={(e) => type(e.target.value)} aria-label={t('phone.input')} placeholder={t('phone.input')} autoComplete="off" autoCapitalize="off" spellCheck={false} enterKeyHint="send" />
          <button type="submit" className="send" aria-label={t('phone.send')}>
            <CornerDownLeft aria-hidden="true" />
          </button>
        </form>
      </div>
      {sheet === 'actions' && <ActionSheet view={view} onCtrlC={() => key(CTRL_C)} onClose={() => setSheet(null)} onRemoved={props.onBack} />}
      {sheet === 'answer' && (
        <Sheet label={t('phone.answer')} onClose={() => setSheet(null)}>
          <InboxPanel waiting={[view]} onSelect={() => setSheet(null)} onFocus={() => setSheet(null)} />
        </Sheet>
      )}
    </>
  );
}

/** 아래에서 올라오는 판. 바탕을 누르거나 Esc로 닫는다. */
function Sheet(props: { label: string; onClose: () => void; children: ReactNode }) {
  useEffect(() => {
    const close = (e: KeyboardEvent) => e.key === 'Escape' && props.onClose();
    document.addEventListener('keydown', close);
    return () => document.removeEventListener('keydown', close);
  }, [props.onClose]);
  return (
    <div className="phone-scrim" onClick={(e) => e.target === e.currentTarget && props.onClose()}>
      <div className="phone-sheet" role="dialog" aria-modal="true" aria-label={props.label}>
        <span className="grip" aria-hidden="true" />
        {props.children}
      </div>
    </div>
  );
}

/** [⋯] 판(목업 ⑤). 지우기는 판 안에서 한 번 더 누른다. 확인 창을 따로 띄우지 않는다. */
function ActionSheet(props: { view: NodeView; onCtrlC: () => void; onClose: () => void; onRemoved: () => void }) {
  const { t } = useTranslation();
  const [confirm, setConfirm] = useState(false);
  const [refusal, setRefusal] = useState<string | null>(null);
  const remove = () => {
    if (!confirm) return setConfirm(true);
    removeNode(props.view.id).then(props.onRemoved, (err: Error) => setRefusal(err.message));
  };
  return (
    <Sheet label={props.view.name} onClose={props.onClose}>
      <h3>{props.view.name}</h3>
      <button
        type="button"
        className="btn"
        onClick={() => {
          props.onCtrlC();
          props.onClose();
        }}
      >
        {t('phone.sendCtrlC')}
      </button>
      <button type="button" className="btn danger" onClick={remove}>
        {confirm ? t('phone.removeConfirm') : t('phone.remove')}
      </button>
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
