import { Search, SquareTerminal } from 'lucide-react';
import { useId, useLayoutEffect, useState, type CSSProperties, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import { AlertLine } from './AlertLine';
import { PROMPT, streamOf, type TargetKey } from './alertTarget';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { RepoView } from './gen/RepoView';
import type { ScreenWait } from './gen/ScreenWait';
import type { WaitTone } from './gen/WaitTone';
import { useEscapeClose, useOutsidePress } from './popover';
import { openMatches, waitSearch } from './waitSearch';

type Current = { node: NodeId; key: TargetKey };
type Row = { node: NodeView; key: TargetKey; wait: ScreenWait | null; depth: number; context: boolean };
type Repo = Pick<RepoView, 'id' | 'name'>;
type Group = { repo: Repo; rows: Row[] };

const KINDS: readonly (WaitTone | null)[] = [null, 'ask', 'done'];
const CHIP: Record<'all' | WaitTone, 'composer.chipAll' | 'composer.chipQuestion' | 'composer.chipDone'> = { all: 'composer.chipAll', ask: 'composer.chipQuestion', done: 'composer.chipDone' };

const MAX_DEPTH = 6;
const live = (node: NodeView) => node.state.kind === 'running' || node.state.kind === 'done';

function treeRows(own: readonly NodeView[], hits: ReadonlySet<NodeId>): Row[] {
  const ids = new Set(own.map((v) => v.id));
  const walk = (parent: NodeId | null, depth: number): Row[] =>
    own
      .filter((v) => (parent === null ? v.parent === null || !ids.has(v.parent) : v.parent === parent))
      .sort((a, b) => a.id - b.id)
      .flatMap((v) => [{ node: v, key: PROMPT, wait: null, depth, context: !hits.has(v.id) }, ...walk(v.id, depth + 1)]);
  return walk(null, 0);
}

function nodeGroups(nodes: readonly NodeView[], repos: readonly Repo[], query: string, current: NodeId): Group[] {
  const words = waitSearch(query)?.words;
  const listed = nodes.filter((v) => live(v) || v.waits.length > 0 || v.id === current);
  const byId = new Map(listed.map((v) => [v.id, v]));
  const hits = new Set(listed.filter((v) => !words || v.name.toLowerCase().includes(words)).map((v) => v.id));
  const kept = new Set<NodeId>();
  for (const id of hits) for (let v = byId.get(id); v && !kept.has(v.id); v = v.parent === null ? undefined : byId.get(v.parent)) kept.add(v.id);
  const shown = listed.filter((v) => kept.has(v.id));
  return repos.map((repo) => ({ repo, rows: treeRows(shown.filter((v) => v.place.repo === repo.id), hits) })).filter((g) => g.rows.length > 0);
}

const alertRows = (waiting: readonly NodeView[], query: string): Row[] => streamOf(openMatches([...waiting], query)).map((s) => ({ node: s.view, key: s.wait.id, wait: s.wait, depth: 0, context: false }));

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
  const found = alertRows(props.waiting, query);
  const counts = { all: found.length, ask: 0, done: 0 };
  for (const r of found) if (r.wait) counts[r.wait.tone] += 1;
  const groups = nodeGroups(props.nodes, props.repos, query, props.current.node);
  const alerts = found.filter((r) => kind === null || r.wait?.tone === kind);
  const rows = [...groups.flatMap((g) => g.rows), ...alerts];
  const isCurrent = (r: Row) => r.node.id === props.current.node && r.key === props.current.key;
  const [cursor, setCursor] = useState<number | null>(() => Math.max(0, rows.findIndex(isCurrent)));
  const at = Math.min(cursor ?? Math.max(0, rows.findIndex((r) => !r.context)), Math.max(rows.length - 1, 0));
  const option = (n: number) => `${id}-${n}`;
  useLayoutEffect(() => {
    document.getElementById(option(at))?.scrollIntoView({ block: 'nearest' });
  }, [at, query, kind]);
  const choose = (r: Row) => props.onPick(r.node.id, r.key);
  const line = (r: Row) => {
    const index = rows.indexOf(r);
    return <TargetRow key={`${r.node.id}-${r.key}`} id={option(index)} row={r} selected={index === at} current={isCurrent(r)} onMove={() => setCursor(index)} onPick={() => choose(r)} />;
  };
  return (
    <div ref={box} className="target-list" style={{ maxHeight: props.room }} role="dialog" aria-label={t('composer.pickTarget')}>
      <div id={`${id}-list`} className="tl-rows" role="listbox" aria-label={t('composer.pickTarget')}>
        {groups.length > 0 && (
          <div role="group" aria-labelledby={`${id}-nodes`}>
            <h2 id={`${id}-nodes`}>{t('composer.sectionNodes')}</h2>
            {groups.map((g) => (
              <div key={g.repo.id} role="group" aria-label={g.repo.name}>
                <h3>{g.repo.name}</h3>
                {g.rows.map(line)}
              </div>
            ))}
          </div>
        )}
        {alerts.length > 0 && (
          <div role="group" aria-labelledby={`${id}-alerts`}>
            <h2 id={`${id}-alerts`}>{t('composer.sectionAlerts')}</h2>
            {alerts.map(line)}
          </div>
        )}
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
                setCursor(null);
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
  const { wait, node } = props.row;
  return (
    <div id={props.id} className="tl-row" role="option" style={{ '--depth': Math.min(props.row.depth, MAX_DEPTH) } as CSSProperties} aria-label={wait ? undefined : t('composer.instruct', { name: node.name })} data-prompt={wait ? undefined : true} data-context={props.row.context || undefined} data-tone={wait?.tone} aria-selected={props.selected} aria-current={props.current || undefined} onPointerDown={(e) => e.preventDefault()} onPointerMove={() => !props.selected && props.onMove()} onClick={props.onPick}>
      {wait ? (
        <AlertLine view={node} wait={wait} />
      ) : (
        <>
          <SquareTerminal className="kind-mark" aria-hidden="true" />
          <span className="al-text">{node.name}</span>
        </>
      )}
    </div>
  );
}
