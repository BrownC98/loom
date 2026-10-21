// 사이드바 찾기 칸(목업 docs/mockups/sidebar-find.html). 일치 목록과 현재 일치는 App이 가진다. 트리가 일치 줄만 남기고, 그래프가 강조한다.
// 이 칸은 입력과 화면 이동만 한다.
import { Search, X } from 'lucide-react';
import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import type { Found } from './findSpots';
import type { RepoId } from './gen/RepoId';
import { flowId, useShowGroups, useShowNodes } from './Graph';

/** 여는 키(Terminal.tsx `findKey`). 툴팁과 `aria-keyshortcuts`에 보인다. */
export const FIND_KEY = 'Ctrl+Shift+F';
export const FIND_ARIA_KEY = 'Control+Shift+F';
/** 입력 칸 id. App이 여는 키·레일 단추로 이 칸에 포커스를 준다. */
export const FIND_INPUT = 'find-input';

/** 빈 `query`는 찾기가 꺼진 상태다. `at`은 `found` 안 현재 일치 번호다. `step`은 다음(1)·이전(-1)이다. */
export function Find(props: {
  query: string;
  found: readonly Found[];
  at: number;
  /** 현재 저장소. 현재 일치가 다른 저장소에 있으면 `onRepo`로 바꾼다. */
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

  // 현재 일치가 바뀌면 그 상자로 화면을 옮긴다. 다른 저장소면 그 저장소로 바꾼다(묶음·노드를 그린 뒤 React Flow가 옮긴다).
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
        // 브라우저가 search 칸에 넣는 지우기 단추는 아래 지우기 단추와 겹친다. 그래서 text 칸이다.
        type="text"
        name="find"
        aria-label={t('find.open')}
        aria-keyshortcuts={FIND_ARIA_KEY}
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
