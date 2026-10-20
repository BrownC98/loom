// 사이드바 트리(docs/mockups/graph-panels.html 1번): 저장소 > 워크트리 묶음 > 노드 > 자식 노드.
// 그래프와 같은 노드 목록을 그린다. 트리만의 상태는 접힌 줄뿐이다.
import { useState, type CSSProperties, type KeyboardEvent, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { agentLabel } from './catalog';
import type { AgentView } from './gen/AgentView';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { RepoId } from './gen/RepoId';
import type { RepoView } from './gen/RepoView';
import { AddRepoPopover } from './AddRepoPopover';
import { useShowNodes } from './Graph';
import i18next from './i18n';
import { groupsOf, inGroup } from './repos';

type Item = { key: string; row: ReactNode; node: NodeId | null; repo?: RepoId; children: Item[] };


function Chevron({ open }: { open: boolean }) {
  return (
    <svg className={open ? 'chev' : 'chev closed'} viewBox="0 0 24 24" aria-hidden="true">
      <path d="M6 9l6 6 6-6" />
    </svg>
  );
}

/** `parent`가 없으면 묶음의 뿌리: 부모가 없거나 부모가 다른 묶음에 있는 노드다. */
function nodeItems(views: NodeView[], parent: NodeId | null, agents: AgentView[]): Item[] {
  return views
    .filter((v) => (parent === null ? !views.some((p) => p.id === v.parent) : v.parent === parent))
    .sort((a, b) => a.id - b.id)
    .map((v) => ({
      key: `node:${v.id}`,
      node: v.id,
      row: (
        <>
          <span className={`dot ${v.state.kind}`} aria-label={v.state.kind} />
          <span className="name">{v.name}</span>
          <span className="sub">{agentLabel(agents, v.agent)}</span>
          {(v.unread > 0 || v.held > 0) && <span className="spacer" />}
          {v.unread > 0 && (
            <span className="unread" aria-label={i18next.t('node.unread', { n: v.unread })}>
              {v.unread}
            </span>
          )}
          {v.held > 0 && (
            <span className="kept" title={i18next.t('node.heldTitle', { n: v.held })}>
              {i18next.t('node.held', { n: v.held })}
            </span>
          )}
        </>
      ),
      children: nodeItems(views, v.id, agents),
    }));
}

export function Tree(props: {
  repos: RepoView[];
  /** 그래프가 그리는 저장소. */
  current: RepoId | null;
  onRepo: (id: RepoId) => void;
  onAdded: (repo: RepoView) => void;
  views: ReadonlyMap<NodeId, NodeView>;
  agents: AgentView[];
  selected: NodeId | null;
  onSelect: (id: NodeId) => void;
}) {
  const { t } = useTranslation();
  const show = useShowNodes();
  const [adding, setAdding] = useState(false);
  const [closed, setClosed] = useState<ReadonlySet<string>>(new Set());
  const repoKey = (id: RepoId) => `repo:${id}`;
  // Tab으로 들어올 때 포커스를 받는 줄(roving tabindex).
  const [active, setActive] = useState('');
  const toggle = (key: string, open: boolean) =>
    setClosed((prev) => {
      const next = new Set(prev);
      if (open) next.delete(key);
      else next.add(key);
      return next;
    });

  const views = [...props.views.values()];
  const roots: Item[] = props.repos.map((repo) => ({
    key: repoKey(repo.id),
    node: null,
    repo: repo.id,
    row: (
      <>
        <span className="name">{repo.name}</span>
        <span className="sub" translate="no" title={repo.path}>
          {repo.path}
        </span>
      </>
    ),
    children: groupsOf(repo, views).map((group) => ({
      key: `group:${group.path}`,
      node: null,
      row: (
        <>
          <span className="branch" translate="no" title={group.path}>
            {group.label}
          </span>
          <span className="sub">{t(group.original ? 'worktree.original' : 'worktree.linked')}</span>
        </>
      ),
      children: nodeItems(inGroup(views, group), null, props.agents),
    })),
  }));
  const visible = new Set<string>();
  const walk = (item: Item) => {
    visible.add(item.key);
    if (!closed.has(item.key)) item.children.forEach(walk);
  };
  roots.forEach(walk);
  const tabStop = visible.has(active) ? active : (roots[0]?.key ?? '');

  const render = (item: Item, depth: number, parentKey: string | null): ReactNode => {
    const open = !closed.has(item.key);
    const branch = item.children.length > 0;
    return (
      <li key={item.key} role="treeitem" aria-expanded={branch ? open : undefined} aria-selected={item.node !== null ? item.node === props.selected : undefined}>
        <button
          className="tree-row"
          style={{ '--depth': depth } as CSSProperties}
          data-key={item.key}
          data-parent={parentKey ?? undefined}
          data-open={branch ? open : undefined}
          tabIndex={item.key === tabStop ? 0 : -1}
          onFocus={() => setActive(item.key)}
          aria-current={(item.node !== null && item.node === props.selected) || (item.repo !== undefined && item.repo === props.current) ? 'true' : undefined}
          onClick={() => {
            // 저장소 줄은 그래프를 그 저장소로 바꾼다. 고른 저장소 줄을 다시 누르면 접고 편다.
            if (item.repo !== undefined && item.repo !== props.current) {
              toggle(item.key, true);
              return props.onRepo(item.repo);
            }
            if (item.node === null) return toggle(item.key, !open);
            show([item.node]);
            props.onSelect(item.node);
          }}
        >
          {branch ? <Chevron open={open} /> : <span className="leaf" />}
          {item.row}
        </button>
        {branch && open && <ul role="group">{item.children.map((child) => render(child, depth + 1, item.key))}</ul>}
      </li>
    );
  };

  // 트리 키보드: ↑↓ 줄 옮기기, → 펴기, ← 접기 또는 부모 줄로, Enter·Space는 단추 기본 동작(고르기).
  const onKeyDown = (e: KeyboardEvent<HTMLUListElement>) => {
    // 보이는 줄 단추, 위에서부터.
    const rows = [...e.currentTarget.querySelectorAll<HTMLButtonElement>('button.tree-row')];
    const at = rows.indexOf(document.activeElement as HTMLButtonElement);
    const current = rows[at];
    const focus = (row: HTMLButtonElement | undefined) => row?.focus();
    if (e.key === 'ArrowDown') focus(rows[at + 1]);
    else if (e.key === 'ArrowUp') focus(rows[Math.max(0, at - 1)]);
    else if (e.key === 'ArrowRight' && current?.dataset.open === 'false') toggle(current.dataset.key!, true);
    else if (e.key === 'ArrowLeft' && current?.dataset.open === 'true') toggle(current.dataset.key!, false);
    else if (e.key === 'ArrowLeft' && current?.dataset.parent) focus(rows.find((r) => r.dataset.key === current.dataset.parent));
    else return;
    e.preventDefault();
  };

  return (
    <>
      <div className="side-head">
        <h2>{t('tree.repos')}</h2>
        <span className="spacer" />
        <button className="btn ghost sm" aria-label={t('tree.addRepo')} title={t('tree.addRepo')} aria-expanded={adding} onClick={() => setAdding(true)}>
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M12 5v14M5 12h14" />
          </svg>
        </button>
        <button className="btn ghost sm" aria-label={t('tree.expandAll')} title={t('tree.expandAll')} onClick={() => setClosed(new Set())}>
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M7 15l5 5 5-5M7 9l5-5 5 5" />
          </svg>
        </button>
      </div>
      <ul className="tree" role="tree" aria-label={t('tree.label')} onKeyDown={onKeyDown}>
        {roots.map((root) => render(root, 0, null))}
      </ul>
      {adding && (
        <AddRepoPopover
          onClose={() => setAdding(false)}
          onAdded={(repo) => {
            setAdding(false);
            props.onAdded(repo);
          }}
        />
      )}
    </>
  );
}
