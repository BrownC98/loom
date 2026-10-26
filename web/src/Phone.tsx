import { ArrowDown, ArrowLeft, ArrowRight, ArrowUp, Bell, Check, ChevronDown, ChevronLeft, ChevronUp, CornerDownLeft, EllipsisVertical, EyeOff, ListTree, Moon, SettingsIcon, Sun, SunMoon, TriangleAlert, type LucideIcon } from 'lucide-react';
import { useEffect, useRef, useState, type FormEvent, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { nodePty, relaunchNode, removeNode } from './api';
import type { Theme } from './gen/Theme';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import { bookmarkToggle } from './BookmarkButton';
import { watchComposition, type Composition } from './composition';
import { InboxPanel, waitCount } from './Inbox';
import { AgentLine, stateLabel } from './NodeCard';
import { NodeText, NoticeChips, NoticeLead, oldestWait, toneOf } from './notice';
import { useNotifyState } from './phoneNotify';
import type { PhoneNotify } from './native';
import { borrowSize, SizingTerminal } from './Terminal';
import { readClipboard } from './touchTerminal';
import { isSheetEntry, MenuSheet, Sheet, type MenuItem } from './sheet';

export type PhoneRoute = { screen: 'inbox' } | { screen: 'nodes' } | { screen: 'node'; id: NodeId; ask?: true };

export function readRoute(hash: string): PhoneRoute {
  if (hash === '#/inbox') return { screen: 'inbox' };
  const node = /^#\/node\/(\d+)(\/ask)?$/.exec(hash);
  if (!node) return { screen: 'nodes' };
  const id = Number(node[1]) as NodeId;
  return node[2] ? { screen: 'node', id, ask: true } : { screen: 'node', id };
}

function currentRoute(): PhoneRoute {
  const route = readRoute(location.hash);
  if (route.screen === 'node' && route.ask) history.replaceState(history.state, '', routeHash(route));
  return route;
}

type NodeEntry = { loomNode: true };

function isNodeEntry(state: unknown): state is NodeEntry {
  return typeof state === 'object' && state !== null && (state as NodeEntry).loomNode === true;
}

function routeHash(route: PhoneRoute): string {
  return route.screen === 'node' ? `#/node/${route.id}` : `#/${route.screen}`;
}

export function usePhoneRoute(): [PhoneRoute, (next: PhoneRoute) => void] {
  const [route, setRoute] = useState(currentRoute);
  useEffect(() => {
    const onHash = () => setRoute(currentRoute());
    window.addEventListener('hashchange', onHash);
    return () => window.removeEventListener('hashchange', onHash);
  }, []);
  const go = (next: PhoneRoute) => {
    if (next.screen === 'node' && !isSheetEntry(history.state)) history.pushState({ loomNode: true } satisfies NodeEntry, '', routeHash(next));
    else history.replaceState(null, '', routeHash(next));
    setRoute(next);
  };
  return [route, go];
}

export function Phone(props: {
  route: PhoneRoute;
  theme: Theme;
  onTheme: (next: Theme) => void;
  onRoute: (next: PhoneRoute) => void;
  waiting: NodeView[];
  nodes: ReadonlyMap<NodeId, NodeView>;
  agentLabel: (view: NodeView) => string;
  online: boolean;
  inbox: ReactNode;
  tree: ReactNode;
  foot?: ReactNode;
}) {
  const { t } = useTranslation();
  const { route } = props;
  const count = waitCount(props.waiting);
  const [askOpen, setAskOpen] = useState(false);
  useEffect(() => {
    if (count === 0) setAskOpen(false);
  }, [count]);
  useEffect(() => {
    if (route.screen === 'node' && route.ask) setAskOpen(true);
  }, [route]);
  const view = route.screen === 'node' ? props.nodes.get(route.id) : undefined;
  if (route.screen === 'node')
    return (
      <div className="phone">
        {view ? (
          <NodeScreen key={view.id} askOpen={askOpen} setAskOpen={setAskOpen} view={view} waiting={props.waiting} agentLabel={props.agentLabel(view)} online={props.online} onBack={() => (isNodeEntry(history.state) ? history.back() : props.onRoute({ screen: 'nodes' }))} onOpen={(id) => props.onRoute({ screen: 'node', id })} />
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
        <SettingsButton theme={props.theme} onTheme={props.onTheme} />
      </header>
      {!props.online && <Offline />}
      <main className="phone-body" aria-label={inbox ? t('phone.inbox') : t('phone.nodes')}>
        {inbox && count === 0 ? (
          <p className="phone-empty center">{t('phone.noNotices')}</p>
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

function SettingsButton(props: { theme: Theme; onTheme: (next: Theme) => void }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return (
    <>
      <button type="button" className="phone-icon" aria-label={t('phone.settings')} aria-haspopup="dialog" onClick={() => setOpen(true)}>
        <SettingsIcon aria-hidden="true" />
      </button>
      {open && <SettingsSheet theme={props.theme} onTheme={props.onTheme} notify={window.loomNotify} onClose={() => setOpen(false)} />}
    </>
  );
}

export function SettingsSheet(props: { theme: Theme; onTheme: (next: Theme) => void; notify: PhoneNotify | undefined; onClose: () => void }) {
  const { t } = useTranslation();
  return (
    <Sheet label={t('phone.settings')} onClose={props.onClose}>
      <h4 className="sheet-section">{t('phone.theme')}</h4>
      {THEME_CHOICES.map((choice) => (
        <button key={choice.value} type="button" className="btn" aria-pressed={choice.value === props.theme} onClick={() => props.onTheme(choice.value)}>
          <choice.icon />
          {t(choice.label)}
          {choice.value === props.theme && <Check className="mark" aria-hidden="true" />}
        </button>
      ))}
      {props.notify && <NotifySwitches bridge={props.notify} />}
    </Sheet>
  );
}

function NotifySwitches(props: { bridge: PhoneNotify }) {
  const { t } = useTranslation();
  const state = useNotifyState(props.bridge);
  const rows = [
    { key: 'ask', label: t('phone.notifyAsk'), hint: t('phone.notifyAskHint'), on: state.ask, set: (on: boolean) => props.bridge.ask(on) },
    { key: 'done', label: t('phone.notifyDone'), hint: t('phone.notifyDoneHint'), on: state.done, set: (on: boolean) => props.bridge.done(on) },
  ];
  return (
    <>
      <h4 className="sheet-section">{t('phone.notify')}</h4>
      {rows.map((row) => (
        <label key={row.key} className="sheet-switch">
          <span className="two">
            {row.label}
            <small data-warn={!state.allowed || undefined}>{state.allowed ? row.hint : t('phone.notifyBlocked')}</small>
          </span>
          <input type="checkbox" role="switch" checked={row.on} onChange={(e) => row.set(e.target.checked)} />
        </label>
      ))}
      {props.bridge.link && (
        <>
          <button type="button" className="btn" onClick={() => props.bridge.link?.()}>
            <EyeOff />
            {t('phone.notifyLink')}
          </button>
          <p className="hint">{t('phone.notifyLinkHint')}</p>
        </>
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

const ARROWS = [
  { key: 'D', label: 'left', icon: ArrowLeft },
  { key: 'A', label: 'up', icon: ArrowUp },
  { key: 'B', label: 'down', icon: ArrowDown },
  { key: 'C', label: 'right', icon: ArrowRight },
] as const;
const ESC = '\x1b';
const CTRL_C = '\x03';

function controlOf(char: string): string {
  const code = char.toUpperCase().charCodeAt(0);
  return code >= 0x40 && code <= 0x5f ? String.fromCharCode(code - 0x40) : char;
}

export function NodeTitle(props: { view: NodeView; agentLabel: string }) {
  return (
    <span className="phone-title">
      <span className="phone-name">
        <b>{props.view.name}</b>
        <NoticeChips view={props.view} />
        {(props.view.state.kind === 'exited' || props.view.state.kind === 'setupFailed') && (
          <span className={`state ${props.view.state.kind}`} aria-live="polite" translate="no">
            {stateLabel(props.view.state)}
          </span>
        )}
      </span>
      <span className="node-line">
        <AgentLine view={props.view} label={props.agentLabel} />
      </span>
    </span>
  );
}

function NodeScreen(props: { askOpen: boolean; setAskOpen: (open: boolean) => void; view: NodeView; waiting: NodeView[]; agentLabel: string; online: boolean; onBack: () => void; onOpen: (id: NodeId) => void }) {
  const { t } = useTranslation();
  const { view } = props;
  const send = useRef<((data: string) => void) | null>(null);
  const paster = useRef<((text: string) => void) | null>(null);
  const [pasteFailed, setPasteFailed] = useState(false);
  const [ctrl, setCtrl] = useState(false);
  const [text, setText] = useState('');
  const box = useRef<HTMLInputElement>(null);
  const composition = useRef<Composition | null>(null);
  useEffect(() => {
    if (!box.current) return;
    const watched = watchComposition(box.current);
    composition.current = watched;
    return () => {
      watched.dispose();
      composition.current = null;
    };
  }, []);
  const [sheet, setSheet] = useState<'actions' | null>(null);
  const { askOpen, setAskOpen } = props;
  const { waiting } = props;
  const head = waiting[0];
  const first = head && oldestWait(head);
  const waits = waiting.flatMap((v) => v.waits);
  const openOther = (id: NodeId) => {
    if (id !== view.id) props.onOpen(id);
  };

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
    const sendBox = () => {
      key(`${box.current?.value ?? text}\r`);
      setText('');
    };
    if (!composition.current?.afterText(sendBox)) sendBox();
  };
  const type = (next: string) => {
    if (ctrl && next.length === text.length + 1 && next.startsWith(text)) return key(controlOf(next.slice(-1)));
    setText(next);
  };

  return (
    <>
      <header className="phone-top">
        <button type="button" className="phone-icon" aria-label={t('phone.back')} onClick={props.onBack}>
          <ChevronLeft aria-hidden="true" />
        </button>
        <NodeTitle view={view} agentLabel={props.agentLabel} />
        <button type="button" className="phone-icon" aria-label={t('phone.actions')} aria-haspopup="dialog" onClick={() => setSheet('actions')}>
          <EllipsisVertical aria-hidden="true" />
        </button>
      </header>
      {!props.online && <Offline />}
      {first && (
        <div className="phone-ask-wrap" data-tone={toneOf(first.kind)}>
          <div className="phone-ask" onClick={() => setAskOpen(!askOpen)}>
            <NoticeLead wait={first} waits={waits} />
            <span className="text">
              <NodeText name={head.name} wait={first} />
            </span>
            <button type="button" className="phone-icon" aria-expanded={askOpen} aria-label={askOpen ? t('phone.askClose') : t('phone.askOpen')} onClick={(e) => (e.stopPropagation(), setAskOpen(!askOpen))}>
              {askOpen ? <ChevronUp aria-hidden="true" /> : <ChevronDown aria-hidden="true" />}
            </button>
          </div>
          <div className="phone-ask-panel" hidden={!askOpen}>
            <InboxPanel waiting={waiting} onSelect={openOther} onFocus={openOther} />
          </div>
        </div>
      )}
      <div className="phone-term" translate="no">
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
          <input ref={box} value={text} onChange={(e) => type(e.target.value)} aria-label={t('phone.input')} placeholder={t('phone.input')} autoComplete="off" autoCapitalize="off" spellCheck={false} enterKeyHint="send" />
          <button type="submit" className="send" aria-label={t('phone.send')}>
            <CornerDownLeft aria-hidden="true" />
          </button>
        </form>
      </div>
      {sheet === 'actions' && <ActionSheet view={view} onCtrlC={() => key(CTRL_C)} onClose={() => setSheet(null)} onRemoved={props.onBack} />}
    </>
  );
}

function ActionSheet(props: { view: NodeView; onCtrlC: () => void; onClose: () => void; onRemoved: () => void }) {
  const { t } = useTranslation();
  const bookmark = bookmarkToggle(props.view);
  const offer = props.view.relaunch;
  const relaunch: MenuItem[] = offer ? [{ label: offer === 'queued' ? t('node.relaunchQueued') : t('node.relaunch'), disabled: offer === 'queued', run: () => relaunchNode(props.view.id) }] : [];
  return (
    <MenuSheet
      title={props.view.name}
      onClose={props.onClose}
      items={[
        { label: t('phone.sendCtrlC'), run: props.onCtrlC },
        ...relaunch,
        { label: bookmark.label, run: bookmark.run },
        { label: t('phone.remove'), danger: true, confirm: true, run: () => removeNode(props.view.id).then(props.onRemoved) },
      ]}
    />
  );
}
