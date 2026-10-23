import { Search, X } from 'lucide-react';
import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import type { Found } from './findSpots';
import type { RepoId } from './gen/RepoId';
import { flowId, useShowGroups, useShowNodes } from './Graph';
import { shortcut } from './platform';

export const FIND_KEY = shortcut('F');
export const FIND_INPUT = 'find-input';

export function Find(props: {
  query: string;
  found: readonly Found[];
  at: number;
  repo: RepoId | null;
  onRepo: (id: RepoId) => void;
  onQuery: (query: string) => void;
  onStep: (step: 1 | -1) => void;
  onClear: () => void;
}) {
  const { t } = useTranslation();
  const show = useShowNodes();
  const showGroups = useShowGroups();
  const current = props.found[props.at];

  const currentKey = current && flowId(current);
  useEffect(() => {
    if (!current) return;
    if (current.repo !== props.repo) props.onRepo(current.repo);
    if (current.kind === 'node') show([current.node]);
    else showGroups([current.path]);
  }, [currentKey]);

  const none = props.query.trim() !== '' && props.found.length === 0;
  return (
    <div className="side-find" role="search">
      <Search aria-hidden />
      <input
        id={FIND_INPUT}
        type="text"
        name="find"
        aria-label={t('find.open')}
        aria-keyshortcuts={FIND_KEY.aria}
        aria-invalid={none}
        placeholder={t('find.open')}
        title={`${t('find.open')} (${FIND_KEY})`}
        autoComplete="off"
        spellCheck={false}
        value={props.query}
        onChange={(e) => props.onQuery(e.target.value)}
        onKeyDown={(e) => {
          if (e.nativeEvent.isComposing) return;
          if (e.key === 'Escape') props.onClear();
          else if (e.key === 'Enter') props.onStep(e.shiftKey ? -1 : 1);
          else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') props.onStep(e.key === 'ArrowDown' ? 1 : -1);
          else return;
          e.preventDefault();
        }}
      />
      <span className={none ? 'count none' : 'count'} aria-live="polite">
        {none ? t('find.none') : props.found.length > 0 ? `${props.at + 1}/${props.found.length}` : ''}
      </span>
      {props.query !== '' && (
        <button className="btn ghost sm icon" aria-label={t('find.clear')} title={`${t('find.clear')} (Esc)`} onClick={props.onClear}>
          <X />
        </button>
      )}
    </div>
  );
}
