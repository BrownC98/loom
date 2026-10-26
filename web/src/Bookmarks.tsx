import { Bookmark, ChevronDown, EllipsisVertical } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { bookmarkToggle } from './BookmarkButton';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { RepoView } from './gen/RepoView';
import { useShowNodes } from './camera';
import { stateClass, stateText } from './NodeCard';
import { MenuSheet, useLongPress, usePhone } from './sheet';
import { groupsOf } from './repos';

export function Bookmarks(props: { views: ReadonlyMap<NodeId, NodeView>; repos: RepoView[]; selected: NodeId | null; onSelect: (id: NodeId) => void; open: boolean; onOpen: (open: boolean) => void }) {
  const { t } = useTranslation();
  const show = useShowNodes();
  const { open, onOpen: setOpen } = props;
  const [refusal, setRefusal] = useState<{ node: NodeId; text: string } | null>(null);
  const phone = usePhone();
  const [menu, setMenu] = useState<NodeId | null>(null);
  const longPress = useLongPress((row) => setMenu(Number(row.dataset.node) as NodeId));
  const menuView = menu === null ? undefined : props.views.get(menu);
  const views = [...props.views.values()];
  const marked = views.filter((v) => v.bookmark !== null).sort((a, b) => a.bookmark! - b.bookmark!);
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
                      data-node={v.id}
                      {...(phone ? longPress : {})}
                      aria-current={v.id === props.selected ? 'true' : undefined}
                      onClick={() => {
                        show([v.id]);
                        props.onSelect(v.id);
                      }}
                    >
                      <span className={`dot ${stateClass(v)}`} aria-label={stateText(v)} />
                      <span className="name">{v.name}</span>
                      <span className="sub">{where(v)}</span>
                    </button>
                    {phone ? (
                      <button type="button" className="row-menu" aria-label={t('tree.rowMenu', { name: v.name })} aria-haspopup="dialog" onClick={() => setMenu(v.id)}>
                        <EllipsisVertical aria-hidden="true" />
                      </button>
                    ) : (
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
                    )}
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
      {menuView && <MenuSheet title={menuView.name} onClose={() => setMenu(null)} items={[{ label: bookmarkToggle(menuView).label, icon: <Bookmark />, run: bookmarkToggle(menuView).run }]} />}
    </section>
  );
}
