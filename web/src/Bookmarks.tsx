// 사이드바 북마크 영역(목업 docs/mockups/bookmarks.html ④).
// 북마크는 데몬 DB에 남는다(`NodeView.bookmark`). 화면은 데몬이 알린 노드 목록만 그린다.
import { Bookmark, ChevronDown } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { bookmarkToggle } from './BookmarkButton';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { RepoView } from './gen/RepoView';
import { useShowNodes } from './Graph';
import { stateClass } from './NodeCard';
import { groupsOf } from './repos';

/** 사이드바 북마크 영역. 모든 저장소의 북마크를 북마크한 차례로 보인다. 줄을 누르면 트리의 노드 줄과 같다. */
export function Bookmarks(props: { views: ReadonlyMap<NodeId, NodeView>; repos: RepoView[]; selected: NodeId | null; onSelect: (id: NodeId) => void; open: boolean; onOpen: (open: boolean) => void }) {
  const { t } = useTranslation();
  const show = useShowNodes();
  const { open, onOpen: setOpen } = props;
  const [refusal, setRefusal] = useState<{ node: NodeId; text: string } | null>(null);
  const views = [...props.views.values()];
  const marked = views.filter((v) => v.bookmark !== null).sort((a, b) => a.bookmark! - b.bookmark!);
  /** `저장소 · 브랜치`. 저장소를 목록에서 뺐으면 브랜치도 모른다. */
  const where = (v: NodeView) => {
    const repo = props.repos.find((r) => r.id === v.place.repo);
    if (!repo) return '';
    const group = groupsOf(repo, views).find((g) => g.path === v.place.worktree);
    return group ? `${repo.name} · ${group.label}` : repo.name;
  };
  return (
    <section className="bookmarks" aria-label={t('bookmark.label')}>
      <div className="side-head">
        <h2>
          <button type="button" className="side-title bookmarks-title" aria-expanded={open} onClick={() => setOpen(!open)}>
            <ChevronDown className={open ? 'chev' : 'chev closed'} />
            {t('bookmark.title')}
            {marked.length > 0 && <span className="bookmarks-count">{marked.length}</span>}
          </button>
        </h2>
      </div>
      {open &&
        (marked.length === 0 ? (
          <p className="bookmarks-empty">{t('bookmark.empty')}</p>
        ) : (
          <ul className="tree bookmarks-list">
            {marked.map((v) => {
              const toggle = bookmarkToggle(v);
              return (
                <li key={v.id}>
                  <div className="tree-line">
                    <button
                      type="button"
                      className="tree-row"
                      aria-current={v.id === props.selected ? 'true' : undefined}
                      onClick={() => {
                        show([v.id]);
                        props.onSelect(v.id);
                      }}
                    >
                      <span className={`dot ${stateClass(v)}`} aria-label={stateClass(v)} />
                      <span className="name">{v.name}</span>
                      <span className="sub">{where(v)}</span>
                    </button>
                    <span className="row-acts">
                      <button
                        type="button"
                        className="act"
                        aria-label={toggle.label}
                        aria-pressed={toggle.pressed}
                        title={toggle.label}
                        onClick={() => {
                          setRefusal(null);
                          toggle.run().catch((err: Error) => setRefusal({ node: v.id, text: err.message }));
                        }}
                      >
                        <Bookmark />
                      </button>
                    </span>
                  </div>
                  {refusal?.node === v.id && (
                    <p className="row-refusal" role="alert">
                      {refusal.text}
                    </p>
                  )}
                </li>
              );
            })}
          </ul>
        ))}
    </section>
  );
}
