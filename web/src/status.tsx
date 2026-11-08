import { X } from 'lucide-react';
import { createContext, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore, type ReactNode, type RefObject } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { getProcessLog } from './api';
import { useShowNodes } from './camera';
import type { EventRecord } from './gen/EventRecord';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { ProcessState } from './gen/ProcessState';
import type { Status } from './gen/Status';
import i18next, { clockTime, duration } from './i18n';
import { asking, bySince, NoticeTags } from './notice';
import { useDismiss } from './popover';

type Links = { views: ReadonlyMap<NodeId, NodeView>; goTo: (id: NodeId) => void; showEvents: (name: string) => void };

const Linked = createContext<Links>({ views: new Map(), goTo: () => {}, showEvents: () => {} });

export function StatusLinks(props: { views: ReadonlyMap<NodeId, NodeView>; onSelect: (id: NodeId) => void; onEvents: (name: string) => void; children: ReactNode }) {
  const show = useShowNodes();
  const latest = useRef({ show, ...props });
  latest.current = { show, ...props };
  const value = useMemo<Links>(
    () => ({
      views: props.views,
      goTo: (id) => {
        latest.current.show([id]);
        latest.current.onSelect(id);
      },
      showEvents: (name) => latest.current.onEvents(name),
    }),
    [props.views],
  );
  return <Linked.Provider value={value}>{props.children}</Linked.Provider>;
}

const TICK_MS = 30 * 1000;
const tickers = new Set<() => void>();
let now = Date.now();
let timer: number | undefined;

function subscribe(listener: () => void) {
  if (tickers.size === 0) {
    now = Date.now();
    timer = window.setInterval(() => {
      now = Date.now();
      tickers.forEach((l) => l());
    }, TICK_MS);
  }
  tickers.add(listener);
  return () => {
    tickers.delete(listener);
    if (tickers.size === 0) clearInterval(timer);
  };
}

const useNow = () => useSyncExternalStore(subscribe, () => now);

export const STATUS_TONE = {
  human: 'ask',
  setupFailed: 'err',
  stopped: 'err',
  limited: 'err',
  working: 'run',
  children: 'wait',
  review: 'wait',
  merge: 'wait',
  answer: 'wait',
  rereview: 'wait',
  job: 'wait',
  deploy: 'wait',
  mail: 'wait',
  idle: 'wait',
  done: 'ok',
  merged: 'ok',
} as const satisfies Record<Status['kind'], string>;

export const isProblem = (view: NodeView) => STATUS_TONE[view.status.kind] === 'err';

const REWORK_FROM = 3;

export function processLabel(process: ProcessState): string {
  const t = i18next.t;
  switch (process.kind) {
    case 'working':
      return t('node.process.working');
    case 'idle':
      return t('node.process.idle');
    case 'asleep':
      return t('node.process.asleep');
    case 'blocked':
      return t('node.process.blocked');
    case 'stopped':
      return process.code === null ? t('node.process.stopped') : t('node.process.stoppedCode', { code: process.code });
  }
}

export function chipLabel(status: Status): string {
  const t = i18next.t;
  switch (status.kind) {
    case 'human':
      return t('wait.label');
    case 'stopped':
      return status.code === null ? t('node.status.chip.stopped') : t('node.status.chip.stoppedCode', { code: status.code });
    case 'children':
      return t('node.status.chip.children', { n: status.names.length });
    case 'setupFailed':
    case 'limited':
    case 'working':
    case 'review':
    case 'merge':
    case 'answer':
    case 'rereview':
    case 'job':
    case 'deploy':
    case 'mail':
    case 'idle':
    case 'done':
    case 'merged':
      return t(`node.status.chip.${status.kind}`);
  }
}

const SHA = 7;

export function statusLine(view: NodeView, parent: string | null): string | null {
  const t = i18next.t;
  const status = view.status;
  switch (status.kind) {
    case 'human':
      return asking(view).sort(bySince)[0]?.summary || null;
    case 'setupFailed':
      return t('node.foot.setupFailed', { code: status.code });
    case 'stopped':
      if (status.code === null) return t('node.foot.daemonRestarted');
      return parent === null ? t('node.foot.stopped', { code: status.code }) : t('node.foot.stoppedSent', { code: status.code, parent });
    case 'limited':
    case 'answer':
      return t(`node.status.line.${status.kind}`);
    case 'working':
      return status.tool;
    case 'children':
      return status.names.join(' · ');
    case 'review':
      return t('node.status.line.review', { reviewers: status.reviewers, judged: status.judged, branch: status.branch });
    case 'merge':
      return status.branch;
    case 'rereview':
      return t('node.status.line.rereview', { branch: status.branch });
    case 'job':
    case 'idle':
    case 'done':
      return view.reply;
    case 'deploy':
      return t('node.status.line.deploy', { target: status.target, sha: status.sha.slice(0, SHA) });
    case 'mail':
      return null;
    case 'merged':
      return t('node.status.line.merged', { target: status.target, sha: status.sha.slice(0, SHA) });
  }
}

function useStatusLine(view: NodeView): { text: string | null; time: string | null } {
  const { views } = useContext(Linked);
  const clock = useNow();
  const parent = view.parent === null ? null : (views.get(view.parent)?.name ?? null);
  return { text: statusLine(view, parent), time: view.since > 0 ? duration(Math.max(0, clock - view.since * 1000)) : null };
}

export function StatusLine(props: { view: NodeView }) {
  const { text, time } = useStatusLine(props.view);
  if (text === null) return null;
  return (
    <div className="sline">
      <span className="sl-text" title={text}>
        {text}
      </span>
      {time && <time>{time}</time>}
    </div>
  );
}

function Pill(props: { status: Status }) {
  const tone = STATUS_TONE[props.status.kind];
  if (tone === 'ask') return null;
  return (
    <span className={`st ${tone}`} aria-live="polite" translate="no">
      {chipLabel(props.status)}
    </span>
  );
}

function ChipBody(props: { view: NodeView; rework: boolean }) {
  const { t } = useTranslation();
  const { view } = props;
  return (
    <>
      <NoticeTags view={view} />
      <Pill status={view.status} />
      {props.rework && view.rework >= REWORK_FROM && <span className="st err">{t('node.status.chip.rework', { n: view.rework })}</span>}
    </>
  );
}

export function StatusPills(props: { view: NodeView; rework?: boolean }) {
  return (
    <span className="chips">
      <ChipBody view={props.view} rework={props.rework === true} />
    </span>
  );
}

export function StatusChips(props: { view: NodeView }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const button = useRef<HTMLButtonElement>(null);
  return (
    <>
      <button ref={button} type="button" className="chips trigger nodrag" aria-haspopup="dialog" aria-expanded={open} title={chipLabel(props.view.status)} onClick={() => setOpen(!open)}>
        <ChipBody view={props.view} rework />
      </button>
      {open && (
        <StatusPopover view={props.view} anchor={button} label={t('node.status.pop.label', { name: props.view.name })} onClose={() => setOpen(false)} />
      )}
    </>
  );
}

const POP_WIDTH = 380;
const POP_GAP = 8;

function Facts(props: { view: NodeView; onGo: (id: NodeId) => void }) {
  const { view } = props;
  const { views } = useContext(Linked);
  const clock = useNow();
  const { status } = view;
  const line = useStatusLine(view).text;
  if (status.kind === 'human') {
    return (
      <Waits>
        <ul className="facts">
          {asking(view)
            .sort(bySince)
            .map((w) => (
              <li key={w.id}>
                <b>{i18next.t(`wait.${w.kind}`)}</b>
                <span className="fact-text">{w.summary}</span>
                <span className="mono">{duration(Math.max(0, clock - w.since * 1000))}</span>
              </li>
            ))}
        </ul>
      </Waits>
    );
  }
  if (status.kind === 'children') {
    const kids = [...views.values()].filter((v) => v.parent === view.id && status.names.includes(v.name));
    return (
      <Waits>
        <ul className="facts">
          {kids.map((kid) => (
            <li key={kid.id}>
              <button type="button" className="child-go" onClick={() => props.onGo(kid.id)}>
                <b>{kid.name}</b>
                <StatusPills view={kid} />
              </button>
            </li>
          ))}
        </ul>
      </Waits>
    );
  }
  return line === null ? null : (
    <Waits>
      <p className="pop-line">{line}</p>
    </Waits>
  );
}

function Waits(props: { children: ReactNode }) {
  const { t } = useTranslation();
  return (
    <section>
      <h4>{t('node.status.pop.waits')}</h4>
      {props.children}
    </section>
  );
}

function History(props: { records: readonly EventRecord[] }) {
  return (
    <ol className="hist">
      {props.records.flatMap((r) =>
        r.entry.kind === 'processChanged' ? (
          <li key={r.id}>
            <time>{clockTime(r.at * 1000)}</time>
            <span>
              {processLabel(r.entry.from)} → {processLabel(r.entry.to)}
            </span>
          </li>
        ) : (
          []
        ),
      )}
    </ol>
  );
}

function StatusPopover(props: { view: NodeView; anchor: RefObject<HTMLButtonElement | null>; label: string; onClose: () => void }) {
  const { t } = useTranslation();
  const { view, onClose } = props;
  const links = useContext(Linked);
  const box = useDismiss<HTMLDivElement>(onClose, { anchor: props.anchor });
  const clock = useNow();
  const [at, setAt] = useState(() => {
    const r = props.anchor.current!.getBoundingClientRect();
    return { top: r.bottom + 6, left: Math.max(POP_GAP, Math.min(r.right - POP_WIDTH, innerWidth - POP_WIDTH - POP_GAP)) };
  });
  const [history, setHistory] = useState<EventRecord[]>([]);
  useEffect(() => {
    let live = true;
    getProcessLog(view.id).then(
      (records) => live && setHistory(records.slice(0, 5)),
      () => {},
    );
    return () => void (live = false);
  }, [view.id]);
  useLayoutEffect(() => {
    const el = box.current!;
    const rect = props.anchor.current!.getBoundingClientRect();
    const below = rect.bottom + 6;
    setAt((prev) => ({ ...prev, top: below + el.offsetHeight > innerHeight - POP_GAP ? Math.max(POP_GAP, rect.top - 6 - el.offsetHeight) : below }));
  }, [history.length]);
  const go = (id: NodeId) => {
    onClose();
    links.goTo(id);
  };
  return createPortal(
    <div ref={box} className="status-pop" role="dialog" aria-label={props.label} style={at}>
      <div className="pop-head">
        <b>{view.name}</b>
        <StatusPills view={view} rework />
        <span className="spacer" />
        <button type="button" className="btn ghost sm icon" aria-label={t('node.status.pop.close')} onClick={onClose}>
          <X />
        </button>
      </div>
      {view.since > 0 && (
        <p className="pop-now">
          {t('node.status.pop.since', { time: clockTime(view.since * 1000) })} · {duration(Math.max(0, clock - view.since * 1000))}
        </p>
      )}
      <Facts view={view} onGo={go} />
      {history.length > 0 && (
        <section>
          <h4>{t('node.status.pop.history')}</h4>
          <History records={history} />
        </section>
      )}
      <div className="pop-foot">
        <button
          type="button"
          className="btn sm"
          onClick={() => {
            onClose();
            links.showEvents(view.name);
          }}
        >
          {t('node.status.pop.events')}
        </button>
      </div>
    </div>,
    document.body,
  );
}
