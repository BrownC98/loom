// 이벤트 콘솔 칸(docs/mockups/graph-panels.html 그림 A). 노드·메시지·받은편지함 알림 이벤트를 최근 것부터 보인다.
// 줄을 누르면 그 이벤트에 관여한 노드를 그래프에서 강조하고 본문 전문을 편다. 강조 상태는 부르는 쪽(App)이 가진다.
import { useEffect, useRef, useState } from 'react';
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';
import { getEventLog } from './api';
import { agentLabel } from './catalog';
import type { AgentView } from './gen/AgentView';
import type { Entry } from './gen/Entry';
import type { EventRecord } from './gen/EventRecord';
import type { Kind } from './gen/Kind';
import type { WaitId } from './gen/WaitId';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { Who } from './gen/Who';
import { useShowNodes } from './Graph';
import { clockTime } from './i18n';
import { stateLabel } from './NodeCard';
import { refusalWithoutName } from './problem';

type Category = 'node' | 'message' | 'notice';

const CATEGORIES: Category[] = ['node', 'message', 'notice'];

function categoryOf(entry: Entry): Category {
  switch (entry.kind) {
    case 'message':
    case 'refused':
      return 'message';
    case 'asked':
    case 'reported':
    case 'answered':
    case 'instructed':
    case 'unanswered':
    case 'confirmed':
      return 'notice';
    default:
      return 'node';
  }
}

/** 알림 줄의 색 계열(목업 docs/mockups/notice-color.html A안). 사람의 답은 그 답이 닫은 알림의 계열을 따른다. */
function familyOf(entry: Entry): 'question' | 'done' | undefined {
  switch (entry.kind) {
    case 'asked':
    case 'answered':
    case 'unanswered':
      return 'question';
    case 'reported':
    case 'instructed':
    case 'confirmed':
      return 'done';
    default:
      return undefined;
  }
}

/** 이벤트에 관여한 노드. 메시지는 보낸·받은 노드, 입양은 자식·옛 부모·새 부모, 생성은 그 노드와 부모다. */
function whoOf(entry: Entry): Who[] {
  switch (entry.kind) {
    case 'created':
      return [entry.node, ...(entry.parent ? [entry.parent] : [])];
    case 'state':
    case 'removed':
    case 'renamed':
    case 'asked':
    case 'reported':
    case 'answered':
    case 'instructed':
    case 'unanswered':
    case 'confirmed':
      return [entry.node];
    case 'adopted':
      return [entry.node, ...(entry.from ? [entry.from] : []), entry.to];
    case 'message':
      return [entry.from, ...(entry.to ? [entry.to] : [])];
    case 'refused':
      return [entry.from];
    case 'accountSwitched':
    case 'noAccountLeft':
      return [];
  }
}

export function involved(entry: Entry): NodeId[] {
  return whoOf(entry).flatMap((w) => (w.id === null ? [] : [w.id]));
}

/** 칸이 열린 동안의 이벤트, 최근 것부터. 열 때 최근 기록을 읽고, 그 뒤는 `onRecord`로 받은 새 기록을 위에 붙인다. */
export function useEventLog(open: boolean): { records: EventRecord[] | null; onRecord: (record: EventRecord) => void } {
  const [records, setRecords] = useState<EventRecord[] | null>(null);
  // 받은 마지막 기록 id. 읽는 동안은 null이다. 그 사이에 온 기록은 읽은 결과에 들어 있다.
  const newest = useRef<number | null>(null);
  const load = () => {
    newest.current = null;
    getEventLog().then((log) => {
      newest.current = log[0]?.id ?? 0;
      setRecords(log);
    });
  };
  useEffect(() => {
    if (open) load();
    else {
      newest.current = null;
      setRecords(null);
    }
  }, [open]);
  const onRecord = (record: EventRecord) => {
    if (newest.current === null || record.id <= newest.current) return;
    // 번호가 건너뛰었으면 웹소켓이 끊긴 사이에 놓친 기록이 있다.
    if (record.id !== newest.current + 1) return load();
    newest.current = record.id;
    setRecords((prev) => prev && [record, ...prev]);
  };
  return { records, onRecord };
}

/** 줄을 펴면 보이는 글. 메시지는 메시지 본문, 알림은 질문·결과·답 전문이다. */
function bodyOf(record: EventRecord, t: TFunction): string | null {
  const { entry } = record;
  switch (entry.kind) {
    case 'asked':
      return entry.text;
    case 'reported':
      return `${entry.summary}\n${t('events.instruction', { text: entry.instruction })}`;
    case 'answered':
    case 'instructed':
      return entry.text || null;
    default:
      return record.body;
  }
}

function KindLabel({ how }: { how: Kind }) {
  const { t } = useTranslation();
  return <span className="kind">{t(`events.how.${how}`)}</span>;
}

function What({ entry, views, agents }: { entry: Entry; views: ReadonlyMap<NodeId, NodeView>; agents: AgentView[] }) {
  const { t } = useTranslation();
  // 지운 노드는 그래프에 없다. 흐린 이름으로 보인다.
  const name = (w: Who) => (
    <span className={w.id !== null && views.has(w.id) ? 'who' : 'gone'} translate="no">
      {w.name}
    </span>
  );
  switch (entry.kind) {
    case 'created':
      return (
        <>
          <span className="kind">{t('events.kind.created')}</span>
          {name(entry.node)}
          <span className="note">
            {entry.parent && <>{t('events.parent', { name: entry.parent.name })} · </>}
            {agentLabel(agents, entry.agent)}
          </span>
        </>
      );
    case 'state':
      return (
        <>
          <span className="kind">{t('events.kind.state')}</span>
          {name(entry.node)}
          <span className="note">
            {stateLabel(entry.from)} → {stateLabel(entry.to)}
          </span>
        </>
      );
    case 'adopted':
      return (
        <>
          <span className="kind">{t('events.kind.adopted')}</span>
          {name(entry.node)}
          <span className="note">
            {entry.from ? entry.from.name : t('events.noParent')} → {entry.to.name}
            {entry.held > 0 && <> · {t('events.movedHeld', { n: entry.held })}</>}
          </span>
        </>
      );
    case 'removed':
      return (
        <>
          <span className="kind">{t('events.kind.removed')}</span>
          {name(entry.node)}
        </>
      );
    case 'renamed':
      return (
        <>
          <span className="kind">{t('events.kind.renamed')}</span>
          <span translate="no">{entry.from}</span> → {name(entry.node)}
        </>
      );
    case 'message':
      return (
        <>
          <KindLabel how={entry.how} />
          {name(entry.from)}
          {entry.to && <>→{name(entry.to)}</>}
          <span className="note">{t(`events.step.${entry.step}`)}</span>
        </>
      );
    case 'refused':
      return (
        <>
          <KindLabel how={entry.how} />
          {name(entry.from)}→{name({ id: [...views.values()].find((v) => v.name === entry.to)?.id ?? null, name: entry.to })}
          <span className="note err">{t('events.refused', { reason: refusalWithoutName(entry.reason) })}</span>
        </>
      );
    case 'asked':
    case 'reported':
    case 'confirmed':
      return (
        <>
          <span className="kind">{t(`events.kind.${entry.kind}`)}</span>
          {name(entry.node)}
          <WaitNote wait={entry.wait} />
        </>
      );
    case 'answered':
    case 'instructed':
      return (
        <>
          <span className="kind">{t(`events.kind.${entry.kind}`)}</span>
          {name(entry.node)}
          <WaitNote wait={entry.wait} extra={entry.images > 0 ? t('events.images', { n: entry.images }) : undefined} />
        </>
      );
    case 'unanswered':
      return (
        <>
          <span className="kind">{t('events.kind.unanswered')}</span>
          {name(entry.node)}
          <WaitNote wait={entry.wait} extra={t(`events.closedBy.${entry.by}`)} />
        </>
      );
    case 'accountSwitched':
    case 'noAccountLeft':
      // 계정 이름은 이 목록이 모른다. 상태 줄 알림이 이름으로 보인다.
      return (
        <>
          <span className="kind">{t(`events.kind.${entry.kind}`)}</span>
          {t(`switch.provider.${entry.provider}`)}
          {entry.kind === 'accountSwitched' && <span className="note">{t('switch.notice.counts', { moved: entry.moved, later: entry.later, failed: entry.failed })}</span>}
        </>
      );
  }
}

/** 받은편지함 번호(`Q12`)와 덧붙일 말. 사람은 받은편지함에서 이 번호를 본다. */
function WaitNote({ wait, extra }: { wait: WaitId; extra?: string }) {
  return (
    <span className="note">
      Q{wait}
      {extra && <> · {extra}</>}
    </span>
  );
}

/** 고른 줄(강조한 줄)만 본문 전문을 편다. 다시 누르면 강조와 함께 접는다(2026-10-22 사람 지시 "기존 메시지 이벤트도 클릭하면 메시지 전문이 펼쳐보이게 수정"). */
function Row(props: { record: EventRecord; current: boolean; onClick: () => void; views: ReadonlyMap<NodeId, NodeView>; agents: AgentView[] }) {
  const { record, current } = props;
  const { t } = useTranslation();
  const body = bodyOf(record, t);
  return (
    <li>
      <button className="ev" data-family={familyOf(record.entry)} aria-current={current || undefined} aria-expanded={body === null ? undefined : current} onClick={props.onClick}>
        <time dateTime={new Date(record.at * 1000).toISOString()}>{clockTime(record.at * 1000)}</time>
        <span className="what">
          <What entry={record.entry} views={props.views} agents={props.agents} />
        </span>
        {body !== null && <span className={current ? 'body open' : 'body'}>{current ? body : body.split('\n', 1)[0]}</span>}
      </button>
    </li>
  );
}

export function EventConsole(props: {
  records: EventRecord[] | null;
  views: ReadonlyMap<NodeId, NodeView>;
  agents: AgentView[];
  highlight: EventRecord | null;
  onHighlight: (record: EventRecord | null) => void;
}) {
  const { t } = useTranslation();
  const show = useShowNodes();
  const [hidden, setHidden] = useState<ReadonlySet<Category>>(new Set());
  const [query, setQuery] = useState('');
  const { onHighlight, highlight } = props;

  useEffect(() => {
    if (highlight === null) return;
    // 터미널 안의 Esc는 터미널 앱이 쓴다.
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !(document.activeElement as HTMLElement | null)?.closest('.xterm')) onHighlight(null);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [highlight, onHighlight]);

  const pick = (record: EventRecord) => {
    if (highlight?.id === record.id) return onHighlight(null);
    onHighlight(record);
    const ids = involved(record.entry).filter((id) => props.views.has(id));
    if (ids.length > 0) show(ids);
  };
  const toggle = (c: Category) =>
    setHidden((prev) => {
      const next = new Set(prev);
      if (!next.delete(c)) next.add(c);
      return next;
    });
  const needle = query.trim();
  const shown = (props.records ?? []).filter(
    (r) => !hidden.has(categoryOf(r.entry)) && (needle === '' || whoOf(r.entry).some((w) => w.name.includes(needle)) || (r.entry.kind === 'refused' && r.entry.to.includes(needle)) || (r.entry.kind === 'renamed' && r.entry.from.includes(needle))),
  );

  return (
    <>
      <p className="dock-head hint">{t('events.order')}</p>
      <div className="filters" role="group" aria-label={t('events.filterKinds')}>
        {CATEGORIES.map((c) => (
          <button key={c} className="chip" aria-pressed={!hidden.has(c)} onClick={() => toggle(c)}>
            {t(`events.category.${c}`)}
          </button>
        ))}
        <input type="search" name="node" aria-label={t('events.filterName')} placeholder={t('events.filterNamePlaceholder')} autoComplete="off" spellCheck={false} value={query} onChange={(e) => setQuery(e.target.value)} />
      </div>
      {props.records === null ? (
        <p className="dock-note">{t('events.reading')}</p>
      ) : props.records.length === 0 ? (
        <p className="dock-note">{t('events.empty')}</p>
      ) : shown.length === 0 ? (
        <p className="dock-note">{t('events.noMatch')}</p>
      ) : (
        <ol className="events" aria-live="polite" aria-label={t('events.list')}>
          {shown.map((r) => (
            <Row key={r.id} record={r} current={highlight?.id === r.id} onClick={() => pick(r)} views={props.views} agents={props.agents} />
          ))}
        </ol>
      )}
    </>
  );
}
