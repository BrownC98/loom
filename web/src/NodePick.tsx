import { Check, Search } from 'lucide-react';
import { useEffect, useId, useState, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import { nameHit, spotsWhere } from './findSpots';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { RepoView } from './gen/RepoView';
import { stateClass, stateText } from './NodeCard';
import { useEscapeClose, useOutsidePress } from './popover';
import { bookmarked, placeName } from './repos';

export function NodePick(props: {
  views: ReadonlyMap<NodeId, NodeView>;
  repos: readonly RepoView[];
  current: NodeId;
  anchor: RefObject<HTMLElement | null>;
  width: number;
  room: number;
  onPick: (id: NodeId) => void;
  onClose: () => void;
  onEscape: () => void;
}) {
  const { t } = useTranslation();
  const id = useId();
  const [query, setQuery] = useState('');
  const box = useEscapeClose<HTMLDivElement>(props.onEscape);
  useOutsidePress(box, props.onClose, props.anchor);
  const views = [...props.views.values()];
  const hit = nameHit(query);
  const marks = bookmarked(views).filter((v) => hit(v.name));
  const all = spotsWhere(props.repos, views, hit).flatMap((f) => (f.kind === 'node' ? (props.views.get(f.node) ?? []) : []));
  const rows = [...marks, ...all];
  const [at, setAt] = useState(() => Math.max(0, rows.findIndex((v) => v.id === props.current)));
  const option = (n: number) => `${id}-${n}`;
  useEffect(() => {
    const row = document.getElementById(option(at));
    const list = row?.parentElement?.parentElement;
    if (!row || !list) return;
    const top = row.offsetTop;
    if (top < list.scrollTop) list.scrollTop = top;
    else if (top + row.offsetHeight > list.scrollTop + list.clientHeight) list.scrollTop = top + row.offsetHeight - list.clientHeight;
  }, [at]);
  const sections = [
    { key: 'marks', label: t('bookmark.title'), rows: marks, from: 0 },
    { key: 'all', label: t('composer.pickAll'), rows: all, from: marks.length },
  ].filter((s) => s.rows.length > 0);
  return (
    <div ref={box} className="node-pick" style={{ width: props.width, maxHeight: props.room }} role="dialog" aria-label={t('composer.pickNode')}>
      <label className="pick-find">
        <Search aria-hidden="true" />
        <input
          autoFocus
          role="combobox"
          aria-expanded="true"
          aria-controls={`${id}-list`}
          aria-activedescendant={rows[at] ? option(at) : undefined}
          aria-label={t('composer.pickSearch')}
          placeholder={t('composer.pickSearch')}
          autoComplete="off"
          spellCheck={false}
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setAt(0);
          }}
          onKeyDown={(e) => {
            if (e.nativeEvent.isComposing) return;
            if (e.key === 'ArrowDown' || e.key === 'ArrowUp') setAt((at + (e.key === 'ArrowDown' ? 1 : -1) + rows.length) % Math.max(rows.length, 1));
            else if (e.key === 'Enter' && rows[at]) props.onPick(rows[at].id);
            else return;
            e.preventDefault();
          }}
        />
      </label>
      {rows.length === 0 ? (
        <p className="pick-none">{t('composer.pickNone')}</p>
      ) : (
        <div id={`${id}-list`} className="pick-list" role="listbox" aria-label={t('composer.pickNode')}>
          {sections.map((s) => (
            <div key={s.key} role="group" aria-labelledby={`${id}-${s.key}`}>
              <h3 id={`${id}-${s.key}`}>{s.label}</h3>
              {s.rows.map((v, i) => {
                const n = s.from + i;
                return (
                  <div key={v.id} id={option(n)} className="pick-row" role="option" aria-selected={n === at} aria-current={v.id === props.current || undefined} onPointerDown={(e) => e.preventDefault()} onPointerMove={() => n !== at && setAt(n)} onClick={() => props.onPick(v.id)}>
                    <span className={`dot ${stateClass(v)}`} aria-label={stateText(v)} />
                    <span className="name">
                      {v.name}
                      <span className="sub">{placeName(props.repos, views, v)}</span>
                    </span>
                    {v.id === props.current && <Check aria-hidden="true" />}
                  </div>
                );
              })}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
