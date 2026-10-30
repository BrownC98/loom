import { Check } from 'lucide-react';
import { memo, useCallback, useEffect, useRef, useState } from 'react';
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';
import { getClosedWaits } from './api';
import { useShowNodes } from './camera';
import { DocText } from './DocText';
import type { Closed } from './gen/Closed';
import type { ClosedWait } from './gen/ClosedWait';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import { dateAndTime } from './i18n';
import { InboxPanel, Instruction, Published, waitCount } from './Inbox';
import { ICON, showsId } from './notice';

type InboxTab = 'new' | 'history';

export function InboxTabs(props: { waiting: NodeView[]; onSelect: (id: NodeId) => void; onFocus: (id: NodeId) => void }) {
  const { t } = useTranslation();
  const [tab, setTab] = useState<InboxTab>('new');
  const n = waitCount(props.waiting);
  const open = props.waiting.flatMap((v) => v.waits.map((w) => w.id)).join(',');
  return (
    <>
      <div className="inbox-tabs">
        <div className="choice" role="tablist" aria-label={t('inbox.tab')}>
          <button type="button" role="tab" aria-selected={tab === 'new'} onClick={() => setTab('new')}>
            {t('inbox.new')}
            {n > 0 && <span className="count">{n}</span>}
          </button>
          <button type="button" role="tab" aria-selected={tab === 'history'} onClick={() => setTab('history')}>
            {t('inbox.history')}
          </button>
        </div>
      </div>
      {tab === 'new' ? <InboxPanel waiting={props.waiting} onSelect={props.onSelect} onFocus={props.onFocus} /> : <History open={open} onSelect={props.onSelect} />}
    </>
  );
}

function History(props: { open: string; onSelect: (id: NodeId) => void }) {
  const { t } = useTranslation();
  const show = useShowNodes();
  const [list, setList] = useState<ClosedWait[] | null>(null);
  const [more, setMore] = useState(false);
  const [refusal, setRefusal] = useState<string | null>(null);
  const loaded = useRef(false);
  useEffect(() => {
    let live = true;
    getClosedWaits().then(
      (page) => {
        if (!live) return;
        setList((now) => [...page.waits.filter((w) => w.seq > (now?.[0]?.seq ?? 0)), ...(now ?? [])]);
        if (!loaded.current) setMore(page.more);
        loaded.current = true;
      },
      (err: Error) => live && setRefusal(err.message),
    );
    return () => {
      live = false;
    };
  }, [props.open]);
  const loadMore = () => {
    const last = list?.at(-1);
    if (!last) return;
    getClosedWaits(last.seq).then(
      (page) => {
        setList((now) => [...(now ?? []), ...page.waits.filter((w) => w.seq < last.seq)]);
        setMore(page.more);
      },
      (err: Error) => setRefusal(err.message),
    );
  };
  const go = (id: NodeId) => {
    show([id]);
    props.onSelect(id);
  };
  const latestGo = useRef(go);
  latestGo.current = go;
  const onGo = useCallback((id: NodeId) => latestGo.current(id), []);
  if (list === null) return refusal ? <p className="inbox-empty refusal">{refusal}</p> : null;
  if (list.length === 0) return <p className="inbox-empty">{t('inbox.historyEmpty')}</p>;
  return (
    <ul className="wait-list history">
      {list.map((w) => (
        <HistoryRow key={w.seq} closed={w} onGo={onGo} onRefusal={setRefusal} t={t} />
      ))}
      {more && (
        <li className="history-more">
          <button type="button" className="btn ghost sm" onClick={loadMore}>
            {t('inbox.historyMore')}
          </button>
        </li>
      )}
      {refusal && (
        <li className="refusal" role="alert">
          {refusal}
        </li>
      )}
    </ul>
  );
}

const HistoryRow = memo(function HistoryRow(props: { closed: ClosedWait; onGo: (id: NodeId) => void; onRefusal: (message: string) => void; t: TFunction }) {
  const { t } = props;
  const { wait, node, chosen } = props.closed;
  const Icon = ICON[wait.tone];
  const choices = wait.choices ?? [];
  return (
    <li className="history-row" data-tone={wait.tone}>
      <div className="entry" data-tone={wait.tone}>
        <div className="top">
          <Icon className="kind-mark" aria-hidden="true" />
          <span className="kind">{t(`wait.${wait.kind}`)}</span>
          {showsId(wait.kind) && <span className="qid">Q{wait.id}</span>}
          {node.id !== null ? (
            <button type="button" className="node" aria-label={t('inbox.jumpLabel', { name: node.name })} onClick={() => node.id !== null && props.onGo(node.id)}>
              {node.name}
            </button>
          ) : (
            <span className="node gone">{node.name}</span>
          )}
          <Published wait={wait} />
        </div>
        {wait.instruction && <Instruction text={wait.instruction} />}
        <div className="wait-body" data-tone={wait.tone}>
          {(wait.text || wait.doc) && (
            <p className="full">
              <DocText parts={wait.parts ?? []} doc={wait.doc} node={node.id} onRefusal={props.onRefusal} />
            </p>
          )}
          {choices.length > 0 && (
            <div className="choices">
              {choices.map((c) => (
                <span key={c.key} className="choice" data-recommended={c.recommended || undefined} data-chosen={c.key === chosen || undefined} data-unchosen={(chosen !== null && c.key !== chosen) || undefined}>
                  <span className="key">{c.key}</span>
                  <span className="label">
                    {c.label}
                    {c.recommended && <span className="mark">{t('inbox.recommended')}</span>}
                  </span>
                  {c.key === chosen && (
                    <span className="chosen">
                      <Check aria-hidden="true" />
                      {t('inbox.chosen')}
                    </span>
                  )}
                </span>
              ))}
            </div>
          )}
          <ClosedLine closed={props.closed} t={t} />
        </div>
      </div>
    </li>
  );
});

function ClosedLine(props: { closed: ClosedWait; t: TFunction }) {
  const { t } = props;
  const { how, chosen, closedAt } = props.closed;
  const said = 'text' in how && how.text !== chosen ? how.text : null;
  return (
    <p className="closed">
      <b>{t(closedLabel(how))}</b> <time dateTime={new Date(closedAt * 1000).toISOString()}>{dateAndTime(closedAt * 1000)}</time>
      {said && <span className="said">{said}</span>}
    </p>
  );
}

function closedLabel(how: Closed) {
  switch (how.kind) {
    case 'answered':
      return 'inbox.closed.answered';
    case 'instructed':
      return 'inbox.closed.instructed';
    case 'confirmed':
      return 'inbox.closed.confirmed';
    case 'nodeRemoved':
      return 'inbox.closed.nodeRemoved';
    case 'unanswered':
      return `inbox.closed.by.${how.by}` as const;
  }
}
