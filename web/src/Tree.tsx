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
import { NameEdit, returnFocus, type Rename } from './NameEdit';
import { Unread } from './NodeCard';
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
function nodeItems(views: NodeView[], parent: NodeId | null, agents: AgentView[], rename: (id: NodeId) => Rename): Item[] {
  return views
    .filter((v) => (parent === null ? !views.some((p) => p.id === v.parent) : v.parent === parent))
    .sort((a, b) => a.id - b.id)
    .map((v) => ({
      key: `node:${v.id}`,
      node: v.id,
      row: (
        <>
          <span className={`dot ${v.state.kind}`} aria-label={v.state.kind} />
          <NameEdit id={v.id} name={v.name} rename={rename(v.id)} />
          <span className="sub">{agentLabel(agents, v.agent)}</span>
          {(v.unread > 0 || v.held > 0) && <span className="spacer" />}
          {v.unread > 0 && <Unread n={v.unread}>{v.unread}</Unread>}
          {v.held > 0 && (
            <span className="kept" title={i18next.t('node.heldTitle', { n: v.held })}>
              {i18next.t('node.held', { n: v.held })}
            </span>
          )}
        </>
      ),
      children: nodeItems(views, v.id, agents, rename),
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
  // 이름을 고치는 노드. 그 줄은 단추 대신 div다. 단추 안에는 입력창을 두지 못한다.
  const [renaming, setRenaming] = useState<NodeId | null>(null);
  const rename = (id: NodeId): Rename => ({
    editing: renaming === id,
    open: () => setRenaming(id),
    close: () => {
      setRenaming(null);
      returnFocus(() => document.querySelector<HTMLElement>(`button.tree-row[data-key="node:${id}"]`));
    },
  });
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
          {/* 긴 브랜치는 한 줄에서 줄임표로 자른다. 툴팁이 전체 이름과 경로를 보인다. 들여쓰기가 워크트리 층을 보이므로 원래 체크아웃에만 꼬리표를 단다(Orca 사이드바). */}
          <span className="branch" translate="no" title={`${group.label}\n${group.path}`}>
            {group.label}
          </span>
          {group.kind !== 'linked' && <span className="tag">{t(`worktree.${group.kind}`)}</span>}
        </>
      ),
      children: nodeItems(inGroup(views, group), null, props.agents, rename),
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
    const head = (
      <>
        {branch ? <Chevron open={open} /> : <span className="leaf" />}
        {item.row}
      </>
    );
    return (
      <li key={item.key} role="treeitem" aria-expanded={branch ? open : undefined} aria-selected={item.node !== null ? item.node === props.selected : undefined}>
        {item.node !== null && item.node === renaming ? (
          <div className="tree-row" style={{ '--depth': depth } as CSSProperties} aria-current={item.node === props.selected ? 'true' : undefined}>
            {head}
          </div>
        ) : (
          <button
            className="tree-row"
            style={{ '--depth': depth } as CSSProperties}
            data-key={item.key}
            data-parent={parentKey ?? undefined}
            data-open={branch ? open : undefined}
            data-node={item.node ?? undefined}
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
            {head}
          </button>
        )}
        {branch && open && <ul role="group">{item.children.map((child) => render(child, depth + 1, item.key))}</ul>}
      </li>
    );
  };

  // 트리 키보드: ↑↓ 줄 옮기기, → 펴기, ← 접기 또는 부모 줄로, F2 노드 이름 바꾸기, Enter·Space는 단추 기본 동작(고르기).
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
    else if (e.key === 'F2' && current?.dataset.node) setRenaming(Number(current.dataset.node));
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
