// 이벤트 콘솔 칸(docs/mockups/graph-panels.html 그림 A). 노드·메시지·받은편지함 알림 이벤트를 최근 것부터 보인다.
// 줄을 누르면 그 이벤트에 관여한 노드를 그래프에서 강조하고 본문 전문을 편다. 강조 상태는 부르는 쪽(App)이 가진다.
import { Fragment, useEffect, useRef, useState } from 'react';
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';
import { getEventLog } from './api';
import { agentLabel } from './catalog';
import type { AgentView } from './gen/AgentView';
import type { Entry } from './gen/Entry';
import type { EventRecord } from './gen/EventRecord';
import type { MessageFacts } from './gen/MessageFacts';
import type { Kind } from './gen/Kind';
import type { MessageKind } from './gen/MessageKind';
import type { Step } from './gen/Step';
import type { TextPart } from './gen/TextPart';
import type { WaitId } from './gen/WaitId';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { Who } from './gen/Who';
import { useShowNodes } from './Graph';
import { DocText } from './DocText';
import { plainText } from './links';
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

type Family = 'question' | 'done' | 'fail';

/** 알림 줄의 색 계열(목업 docs/mockups/notice-color.html A안). 사람의 답은 그 답이 닫은 알림의 계열을 따른다. 메시지는 종류를 따른다(목업 docs/mockups/typed-messages.html M17 ③). */
function familyOf(entry: Entry): Family | undefined {
  switch (entry.kind) {
    case 'message':
      return entry.what && kindFamily(entry.what);
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

function kindFamily(what: MessageKind): Family | undefined {
  switch (what.kind) {
    case 'ask':
    case 'answer':
      return 'question';
    case 'done':
    case 'merged':
      return 'done';
    case 'verdict':
      return what.judgment === 'pass' ? 'done' : 'fail';
    case 'rework':
    case 'failed':
    case 'setupFailed':
      return 'fail';
    case 'exited':
      return what.code !== null && what.code !== 0 ? 'fail' : undefined;
    case 'note':
    case 'ready':
    case 'merging':
    case 'removed':
    case 'adopted':
    case 'humanInput':
    case 'rulesChanged':
      return undefined;
  }
}

/** 종류의 화면 이름. 메모의 조정·중단만 뒤에 붙인다(「메모 · 조정」). */
function kindLabel(what: MessageKind, how: Kind, t: TFunction): string {
  switch (what.kind) {
    case 'note':
      return how === 'queue' ? t('events.type.note') : `${t('events.type.note')} · ${t(`events.how.${how}`)}`;
    case 'verdict':
      return t(`events.type.verdict.${what.judgment}`);
    default:
      return t(`events.type.${what.kind}`);
  }
}

/** 접힌 줄에 보이는 칸 요약. 본문이 아닌 칸이 뜻을 가진 종류만 있다. */
function kindSummary(what: MessageKind, t: TFunction): string | null {
  switch (what.kind) {
    case 'ready':
    case 'verdict':
    case 'merging':
    case 'rework':
    case 'failed':
      return `${what.branch} · ${what.head}`;
    case 'merged':
      return `${what.branch} · ${what.sha}`;
    case 'exited':
    case 'setupFailed':
      return `${what.node} · ${what.code === null ? t('events.codeUnknown') : t('events.code', { code: what.code })}`;
    case 'removed':
      return what.node;
    case 'adopted':
      return `${what.node} → ${what.parent}`;
    case 'rulesChanged':
      return what.skills.join(', ');
    case 'ask':
    case 'answer':
    case 'done':
    case 'note':
    case 'humanInput':
      return null;
  }
}

/** 편 줄에서 본문 칸의 이름. */
function bodyName(what: MessageKind): 'ask' | 'answer' | 'done' | 'next' | 'output' | 'human' | 'text' {
  switch (what.kind) {
    case 'ask':
    case 'answer':
    case 'done':
      return what.kind;
    case 'ready':
    case 'verdict':
    case 'merging':
    case 'merged':
    case 'rework':
    case 'failed':
      return 'next';
    case 'setupFailed':
      return 'output';
    case 'humanInput':
      return 'human';
    case 'note':
    case 'exited':
    case 'removed':
    case 'adopted':
    case 'rulesChanged':
      return 'text';
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
    case 'relaunched':
    case 'asked':
    case 'reported':
    case 'answered':
    case 'instructed':
    case 'unanswered':
    case 'confirmed':
    case 'nudged':
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

/** 줄을 펴면 보이는 글의 조각. 메시지는 메시지 본문, 알림은 질문·결과·답 전문이다. 없으면 null이다. */
function bodyOf(record: EventRecord, t: TFunction): TextPart[] | null {
  const { entry, parts } = record;
  switch (entry.kind) {
    case 'reported':
      return [...parts, { part: 'text', text: `\n${t('events.instruction', { text: entry.instruction })}` }];
    case 'asked':
    case 'answered':
    case 'instructed':
    case 'message':
      return parts.length > 0 ? parts : null;
    default:
      return null;
  }
}

function KindLabel({ how }: { how: Kind }) {
  const { t } = useTranslation();
  return <span className="kind">{t(`events.how.${how}`)}</span>;
}

function What({ entry, facts, views, agents }: { entry: Entry; facts: MessageFacts | null; views: ReadonlyMap<NodeId, NodeView>; agents: AgentView[] }) {
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
    case 'relaunched':
      // 실패한 줄은 데몬의 문장을 붙인다. 턴 끝에 다시 띄우다 실패하면 사람은 여기서만 본다.
      return (
        <>
          <span className="kind">{t('events.kind.relaunched')}</span>
          {name(entry.node)}
          {entry.error !== null && <span className="note err">{entry.error}</span>}
        </>
      );
    case 'nudged':
      return (
        <>
          <span className="kind">{t('events.kind.nudged')}</span>
          {name(entry.node)}
          <span className="note">{t(`events.turnCheck.${entry.check}`)}</span>
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
      return entry.what && facts ? (
        <>
          <span className={`kind ${toneClass(entry.what)}`}>{kindLabel(entry.what, entry.how, t)}</span>
          {facts.from_loom ? (
            <span className="loomw" translate="no">
              loom
            </span>
          ) : (
            name(entry.from)
          )}
          {entry.to && <>→{name(entry.to)}</>}
          <span className="note">{(facts.from_loom && kindSummary(entry.what, t)) || stepNote(entry.what, facts.id, entry.step, t)}</span>
        </>
      ) : (
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
          {entry.what ? <span className="kind k-fail">{kindLabel(entry.what, entry.how, t)}</span> : <KindLabel how={entry.how} />}
          {name(entry.from)}→{name({ id: [...views.values()].find((v) => v.name === entry.to)?.id ?? null, name: entry.to })}
          {entry.reason.code === 'notAncestor' ? (
            // 사람 Q603 A: main이 앞선 것은 흔한 정상 흐름이라 오류 색으로 보이지 않는다.
            <span className="note">{refusalWithoutName(entry.reason)}</span>
          ) : (
            <span className="note err">{t('events.refused', { reason: refusalWithoutName(entry.reason) })}</span>
          )}
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

const TONE: Record<Family, string> = { question: 'k-q', done: 'k-ok', fail: 'k-fail' };

const toneClass = (what: MessageKind) => {
  const family = kindFamily(what);
  return family ? TONE[family] : '';
};

/** 줄 머리 끝의 전달 단계. 질문은 그 번호, 답은 답한 질문 번호를 앞에 둔다. loom 알림은 이 말 대신 칸 요약이다(목업 M17 ① 「종료」 줄). */
function stepNote(what: MessageKind, id: number, step: Step, t: TFunction): string {
  const stepText = t(`events.step.${step}`);
  switch (what.kind) {
    case 'ask':
      return `#${id} · ${stepText}`;
    case 'answer':
      return `${t('events.answerTo', { ref: what.question.in === 'message' ? `#${what.question.id}` : `Q${what.question.id}` })} · ${stepText}`;
    default:
      return stepText;
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
  const { entry } = record;
  const { t } = useTranslation();
  const [refusal, setRefusal] = useState<string | null>(null);
  const body = bodyOf(record, t);
  const what = entry.kind === 'message' ? entry.what : undefined;
  const typed = what !== undefined && record.message !== null;
  // 편 본문은 링크를 품으므로 줄 단추 밖에 둔다. 단추 안에 단추를 둘 수 없다(목업 docs/mockups/doc-reopen.html M13 ①②).
  const doc = entry.kind === 'asked' || entry.kind === 'reported' ? entry.doc : null;
  const node = whoOf(entry)[0]?.id ?? null;
  const summary = what ? kindSummary(what, t) : null;
  // loom 알림의 칸 요약은 줄 머리에 있다.
  const preview = summary !== null ? (record.message?.from_loom ? null : summary) : body === null ? null : plainText(body).split('\n', 1)[0];
  const expandable = body !== null || doc !== null || typed;
  return (
    <li data-family={familyOf(entry)}>
      <button className="ev" aria-current={current || undefined} aria-expanded={expandable ? current : undefined} onClick={props.onClick}>
        <time dateTime={new Date(record.at * 1000).toISOString()}>{clockTime(record.at * 1000)}</time>
        <span className="what">
          <What entry={entry} facts={record.message} views={props.views} agents={props.agents} />
        </span>
        {!current && preview !== null && preview !== '' && <span className="body">{preview}</span>}
      </button>
      {current && what && record.message ? (
        <dl className="fields">
          {record.message.fields.map((field) => (
            <Fragment key={field.name}>
              <dt>{t(`events.field.${field.name}`)}</dt>
              <dd>
                <DocText parts={field.parts} doc={null} node={node} onRefusal={setRefusal} />
              </dd>
            </Fragment>
          ))}
          {body !== null && (
            <>
              <dt>{t(`events.bodyName.${bodyName(what)}`)}</dt>
              <dd className="text">
                <DocText parts={body} doc={null} node={node} onRefusal={setRefusal} />
                {record.choices.length > 0 && (
                  <span className="choices">
                    {record.choices.map((c) => (
                      <span key={c.key} className="choice" data-recommended={c.recommended || undefined}>
                        <span className="key">{c.key}</span>
                        {c.label}
                      </span>
                    ))}
                  </span>
                )}
              </dd>
            </>
          )}
          {refusal && <dd className="note err">{refusal}</dd>}
        </dl>
      ) : (
        current &&
        (body !== null || doc !== null) && (
          <p className="body">
            <DocText parts={body ?? []} doc={doc} node={node} onRefusal={setRefusal} />
            {refusal && <span className="note err">{`\n${refusal}`}</span>}
          </p>
        )
      )}
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
