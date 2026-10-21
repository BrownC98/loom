// 그래프 찾기 막대(목업 docs/mockups/search.html). 일치 목록과 현재 일치는 App이 가진다. 이 막대는 입력과 화면 이동만 한다.
import { ChevronDown, ChevronUp, Search, X } from 'lucide-react';
import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import type { Found } from './findSpots';
import type { RepoId } from './gen/RepoId';
import { flowId, useShowGroups, useShowNodes } from './Graph';
import { findKey } from './Terminal';

/** 여는 키(Terminal.tsx `findKey`). 툴팁과 `aria-keyshortcuts`에 보인다. */
const FIND_KEY = 'Ctrl+Shift+F';
const FIND_ARIA_KEY = 'Control+Shift+F';

/** `query === null`이면 닫혀 단추만 보인다. `at`은 `found` 안 현재 일치 번호다. `step`은 다음(1)·이전(-1)이다. */
export function Find(props: {
  query: string | null;
  found: readonly Found[];
  at: number;
  /** 현재 저장소. 현재 일치가 다른 저장소에 있으면 `onRepo`로 바꾼다. */
  repo: RepoId | null;
  onRepo: (id: RepoId) => void;
  onQuery: (query: string) => void;
  onStep: (step: 1 | -1) => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const input = useRef<HTMLInputElement>(null);
  const show = useShowNodes();
  const showGroups = useShowGroups();
  const { query } = props;
  const open = query !== null;
  const current = props.found[props.at];

  // 터미널 안에서도 연다(터미널은 이 키를 창으로 올려 보낸다). 열려 있으면 입력 글자를 모두 고른다.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!findKey(e)) return;
      e.preventDefault();
      if (!open) props.onQuery('');
      else input.current?.select();
      input.current?.focus();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  // 현재 일치가 바뀌면 그 상자로 화면을 옮긴다. 다른 저장소면 그 저장소로 바꾼다(묶음·노드를 그린 뒤 React Flow가 옮긴다).
  const currentKey = current && flowId(current);
  useEffect(() => {
    if (!current) return;
    if (current.repo !== props.repo) props.onRepo(current.repo);
    if (current.kind === 'node') show([current.node]);
    else showGroups([current.path]);
  }, [currentKey]);

  if (query === null) {
    return (
      <button className="btn icon find-open" aria-label={t('find.open')} aria-keyshortcuts={FIND_ARIA_KEY} title={`${t('find.open')} (${FIND_KEY})`} onClick={() => props.onQuery('')}>
        <Search />
      </button>
    );
  }
  const none = query.trim() !== '' && props.found.length === 0;
  return (
    <div className="find" role="search">
      <Search aria-hidden />
      <input
        ref={input}
        autoFocus
        // 브라우저가 search 칸에 넣는 지우기 단추는 닫기 단추와 헷갈린다. 그래서 text 칸이다.
        type="text"
        name="find"
        aria-label={t('find.placeholder')}
        aria-invalid={none}
        placeholder={t('find.placeholder')}
        autoComplete="off"
        spellCheck={false}
        value={query}
        onChange={(e) => props.onQuery(e.target.value)}
        onKeyDown={(e) => {
          if (e.nativeEvent.isComposing) return;
          if (e.key === 'Escape') props.onClose();
          else if (e.key === 'Enter') props.onStep(e.shiftKey ? -1 : 1);
          else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') props.onStep(e.key === 'ArrowDown' ? 1 : -1);
          else return;
          e.preventDefault();
        }}
      />
      <span className={none ? 'count none' : 'count'} aria-live="polite">
        {none ? t('find.none') : props.found.length > 0 ? `${props.at + 1}/${props.found.length}` : ''}
      </span>
      <button className="btn ghost sm icon" aria-label={t('find.prev')} title={`${t('find.prev')} (Shift+Enter)`} disabled={props.found.length === 0} onClick={() => props.onStep(-1)}>
        <ChevronUp />
      </button>
      <button className="btn ghost sm icon" aria-label={t('find.next')} title={`${t('find.next')} (Enter)`} disabled={props.found.length === 0} onClick={() => props.onStep(1)}>
        <ChevronDown />
      </button>
      <button className="btn ghost sm icon" aria-label={t('find.close')} title={`${t('find.close')} (Esc)`} onClick={props.onClose}>
        <X />
      </button>
    </div>
  );
}
