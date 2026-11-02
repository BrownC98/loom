import { Check, Search } from 'lucide-react';
import { useId, useLayoutEffect, useRef, useState, type RefObject } from 'react';
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
  above: boolean;
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
  const sectionsFor = (q: string) => {
    const hit = nameHit(q);
    const marks = { key: 'marks', label: t('bookmark.title'), rows: bookmarked(views).filter((v) => hit(v.name)), from: 0 };
    const all = { key: 'all', label: t('composer.pickAll'), rows: spotsWhere(props.repos, views, hit).flatMap((f) => (f.kind === 'node' ? (props.views.get(f.node) ?? []) : [])), from: 0 };
    const shown = (props.above ? [all, marks] : [marks, all]).filter((s) => s.rows.length > 0);
    let from = 0;
    for (const s of shown) {
      s.from = from;
      from += s.rows.length;
    }
    return shown;
  };
  const nearest = (shown: ReturnType<typeof sectionsFor>) => (props.above ? Math.max(0, shown.reduce((n, s) => n + s.rows.length, 0) - 1) : 0);
  const sections = sectionsFor(query);
  const rows = sections.flatMap((s) => s.rows);
  const [at, setAt] = useState(() => {
    const mark = sections.find((s) => s.key === 'marks');
    const inMarks = mark?.rows.findIndex((v) => v.id === props.current) ?? -1;
    if (mark && inMarks >= 0) return mark.from + inMarks;
    const found = rows.findIndex((v) => v.id === props.current);
    return found >= 0 ? found : nearest(sections);
  });
  const option = (n: number) => `${id}-${n}`;
  const opened = useRef(false);
  useLayoutEffect(() => {
    const row = document.getElementById(option(at));
    const list = row?.parentElement?.parentElement;
    if (!row || !list) return;
    if (!opened.current) {
      opened.current = true;
      list.scrollTop = props.above ? list.scrollHeight : 0;
    }
    const top = row.offsetTop;
    if (top < list.scrollTop) list.scrollTop = top;
    else if (top + row.offsetHeight > list.scrollTop + list.clientHeight) list.scrollTop = top + row.offsetHeight - list.clientHeight;
  }, [at, query]);
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
            setAt(nearest(sectionsFor(e.target.value)));
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
