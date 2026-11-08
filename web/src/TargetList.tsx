import { Search, SquareTerminal } from 'lucide-react';
import { useId, useLayoutEffect, useState, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import { PROMPT, type TargetKey } from './alertTarget';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { RepoView } from './gen/RepoView';
import type { ScreenWait } from './gen/ScreenWait';
import type { WaitTone } from './gen/WaitTone';
import { clockTime } from './i18n';
import { bySince, ICON, NoticeBadges, showsId } from './notice';
import { useEscapeClose, useOutsidePress } from './popover';
import { openMatches, waitSearch } from './waitSearch';

type Current = { node: NodeId; key: TargetKey };
type Row = { node: NodeView; key: TargetKey; wait: ScreenWait | null };
type Repo = Pick<RepoView, 'id' | 'name'>;
type Head = { kind: 'node'; node: NodeView } | { kind: 'repo'; repo: Repo };
type Group = { head: Head; rows: Row[] };

const KINDS: readonly (WaitTone | null)[] = [null, 'ask', 'done'];
const CHIP: Record<'all' | WaitTone, 'composer.chipAll' | 'composer.chipQuestion' | 'composer.chipDone'> = { all: 'composer.chipAll', ask: 'composer.chipQuestion', done: 'composer.chipDone' };

const promptRow = (node: NodeView): Row => ({ node, key: PROMPT, wait: null });
const live = (node: NodeView) => node.state.kind === 'running' || node.state.kind === 'done';
const groupKey = (head: Head) => (head.kind === 'node' ? `n${head.node.id}` : `r${head.repo.id}`);

function groupsOf(waiting: readonly NodeView[], query: string, kind: WaitTone | null, nodes: readonly NodeView[], repos: readonly Repo[], current: NodeId): Group[] {
  const groups = openMatches([...waiting], query)
    .map((v) => ({ node: waiting.find((w) => w.id === v.id) ?? v, waits: v.waits.filter((w) => kind === null || w.tone === kind) }))
    .filter((g) => g.waits.length > 0)
    .map((g): Group => ({ head: { kind: 'node', node: g.node }, rows: [...g.waits.sort(bySince).map((wait): Row => ({ node: g.node, key: wait.id, wait })), promptRow(g.node)] }));
  if (kind !== null) return groups;
  const listed = new Set(groups.map((g) => g.rows[0].node.id));
  const words = waitSearch(query)?.words;
  const others = nodes.filter((v) => !listed.has(v.id) && (live(v) || v.id === current) && (!words || v.name.toLowerCase().includes(words)));
  for (const repo of repos) {
    const rows = others.filter((v) => v.place.repo === repo.id).map(promptRow);
    if (rows.length > 0) groups.push({ head: { kind: 'repo', repo }, rows });
  }
  return groups;
}

export function TargetList(props: {
  waiting: readonly NodeView[];
  nodes: readonly NodeView[];
  repos: readonly Repo[];
  current: Current;
  anchor: RefObject<HTMLElement | null>;
  room: number;
  onPick: (node: NodeId, key: TargetKey) => void;
  onClose: () => void;
  onEscape: () => void;
}) {
  const { t } = useTranslation();
  const id = useId();
  const [query, setQuery] = useState('');
  const [kind, setKind] = useState<WaitTone | null>(null);
  const box = useEscapeClose<HTMLDivElement>(props.onEscape);
  useOutsidePress(box, props.onClose, props.anchor);
  const matched = openMatches([...props.waiting], query);
  const counts = { all: matched.reduce((n, v) => n + v.waits.length, 0), ask: 0, done: 0 };
  for (const w of matched.flatMap((v) => v.waits)) counts[w.tone] += 1;
  const groups = groupsOf(props.waiting, query, kind, props.nodes, props.repos, props.current.node);
  const rows = groups.flatMap((g) => g.rows);
  const isCurrent = (r: Row) => r.node.id === props.current.node && r.key === props.current.key;
  const [cursor, setCursor] = useState(() => Math.max(0, rows.findIndex(isCurrent)));
  const at = Math.min(cursor, Math.max(rows.length - 1, 0));
  const option = (n: number) => `${id}-${n}`;
  useLayoutEffect(() => {
    document.getElementById(option(at))?.scrollIntoView({ block: 'nearest' });
  }, [at, query, kind]);
  const choose = (r: Row) => props.onPick(r.node.id, r.key);
  let n = -1;
  return (
    <div ref={box} className="target-list" style={{ maxHeight: props.room }} role="dialog" aria-label={t('composer.pickTarget')}>
      <div id={`${id}-list`} className="tl-rows" role="listbox" aria-label={t('composer.pickTarget')}>
        {groups.map((g) => (
          <div key={groupKey(g.head)} role="group" aria-labelledby={`${id}-${groupKey(g.head)}`}>
            <h3 id={`${id}-${groupKey(g.head)}`}>
              {g.head.kind === 'node' ? (
                <>
                  <b>{g.head.node.name}</b>
                  <NoticeBadges waits={g.head.node.waits} />
                </>
              ) : (
                g.head.repo.name
              )}
            </h3>
            {g.rows.map((r) => {
              const index = (n += 1);
              return <TargetRow key={`${r.node.id}-${r.key}`} id={option(index)} row={r} selected={index === at} current={isCurrent(r)} onMove={() => setCursor(index)} onPick={() => choose(r)} />;
            })}
          </div>
        ))}
        {rows.length === 0 && <p className="tl-none">{t('inbox.noMatch')}</p>}
      </div>
      <div className="tl-foot">
        <div className="tl-find">
          <label className="tl-search">
            <Search aria-hidden="true" />
            <input
              autoFocus
              role="combobox"
              aria-expanded="true"
              aria-controls={`${id}-list`}
              aria-activedescendant={rows[at] ? option(at) : undefined}
              aria-label={t('inbox.search')}
              placeholder={t('inbox.searchPlaceholder')}
              autoComplete="off"
              spellCheck={false}
              value={query}
              onChange={(e) => {
                setQuery(e.target.value);
                setCursor(0);
              }}
              onKeyDown={(e) => {
                if (e.nativeEvent.isComposing) return;
                if (e.key === 'ArrowDown' || e.key === 'ArrowUp') setCursor((at + (e.key === 'ArrowDown' ? 1 : -1) + rows.length) % Math.max(rows.length, 1));
                else if (e.key === 'Enter' && rows[at]) choose(rows[at]);
                else return;
                e.preventDefault();
              }}
            />
            <span className="tl-hint">{t('composer.listHint')}</span>
          </label>
          {KINDS.map((k) => {
            const name = k ?? 'all';
            return (
              <button key={name} type="button" className="tl-chip" aria-pressed={kind === k} onClick={() => setKind(k)}>
                {t(CHIP[name], { count: counts[name] })}
              </button>
            );
          })}
        </div>
      </div>
    </div>
  );
}

function TargetRow(props: { id: string; row: Row; selected: boolean; current: boolean; onMove: () => void; onPick: () => void }) {
  const { t } = useTranslation();
  const { wait } = props.row;
  const Icon = wait ? ICON[wait.tone] : SquareTerminal;
  return (
    <div id={props.id} className="tl-row" role="option" data-prompt={wait ? undefined : true} data-tone={wait?.tone} aria-selected={props.selected} aria-current={props.current || undefined} onPointerDown={(e) => e.preventDefault()} onPointerMove={() => !props.selected && props.onMove()} onClick={props.onPick}>
      <Icon className="kind-mark" aria-hidden="true" />
      {wait ? (
        <>
          {showsId(wait.kind) ? <span className="qid">Q{wait.id}</span> : <span className="tl-kind">{t(`wait.${wait.kind}`)}</span>}
          <span className="tl-text">{wait.summary}</span>
          <time className="tl-time" dateTime={new Date(wait.since * 1000).toISOString()}>
            {clockTime(wait.since * 1000)}
          </time>
        </>
      ) : (
        <span className="tl-text">{t('composer.instruct', { name: props.row.node.name })}</span>
      )}
    </div>
  );
}
