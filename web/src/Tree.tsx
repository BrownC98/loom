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
import { groupParents, groupsOf, inGroup, type Group } from './repos';

type Item = { key: string; row: ReactNode; node: NodeId | null; repo?: RepoId; children: Item[] };

/** 들여쓰기 깊이의 화면 상한. 더 깊은 줄은 같은 자리에 두어 이름이 보일 폭을 지킨다(styles.css `--step` 12px × 8 = 96px). */
const MAX_VISUAL_DEPTH = 8;


function Chevron({ open }: { open: boolean }) {
  return (
    <svg className={open ? 'chev' : 'chev closed'} viewBox="0 0 24 24" aria-hidden="true">
      <path d="M6 9l6 6 6-6" />
    </svg>
  );
}

/** 줄 폭 안에서 이름 앞에 오는 git-branch 아이콘(묶음 줄). */
function BranchIcon() {
  return (
    <svg className="icon" viewBox="0 0 24 24" aria-hidden="true">
      <line x1="6" x2="6" y1="3" y2="15" />
      <circle cx="18" cy="6" r="3" />
      <circle cx="6" cy="18" r="3" />
      <path d="M18 9a9 9 0 0 1-9 9" />
    </svg>
  );
}

/** 부모가 나무의 부모 묶음 밖에 있는 노드의 표시(corner-left-up). `title`이 부모 노드와 묶음을 말한다. */
function FromIcon({ title }: { title: string }) {
  return (
    <svg className="from" viewBox="0 0 24 24" role="img" aria-label={title}>
      <title>{title}</title>
      <polyline points="14 9 9 4 4 9" />
      <path d="M20 20h-7a4 4 0 0 1-4-4V4" />
    </svg>
  );
}

/** 노드 줄의 `부모: … · …` 문구를 돌려준다. 문구가 없으면 표시도 없다. */
type ParentNote = (v: NodeView) => string | null;

/** `parent`가 없으면 묶음의 뿌리: 부모가 없거나 부모가 다른 묶음에 있는 노드다. */
function nodeItems(views: NodeView[], parent: NodeId | null, agents: AgentView[], rename: (id: NodeId) => Rename, note: ParentNote): Item[] {
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
          {note(v) !== null && <FromIcon title={note(v)!} />}
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
      children: nodeItems(views, v.id, agents, rename, note),
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
  /** 저장소의 묶음 줄: 뿌리 묶음만 돌려주고, 자식 묶음은 `groupItem`이 부모 묶음 아래에 단다. */
  const rootGroups = (repo: RepoView): Item[] => {
    const groups = groupsOf(repo, views);
    const parents = groupParents(groups, views);
    const note = (at: number): ParentNote => (v) => {
      const parent = v.parent === null ? undefined : props.views.get(v.parent);
      const from = parent === undefined ? -1 : groups.findIndex((g) => g.path === parent.place.worktree);
      return parent !== undefined && from >= 0 && from !== at && from !== parents[at] ? t('tree.parent', { node: parent.name, group: groups[from].label }) : null;
    };
    const groupItem = (group: Group, at: number): Item => ({
      key: `group:${group.path}`,
      node: null,
      row: (
        <>
          <BranchIcon />
          {/* 긴 브랜치는 한 줄에서 줄임표로 자른다. 툴팁이 전체 이름과 경로를 보인다. 들여쓰기가 워크트리 층을 보이므로 원래 체크아웃에만 꼬리표를 단다(Orca 사이드바). */}
          <span className="branch" translate="no" title={`${group.label}\n${group.path}`}>
            {group.label}
          </span>
          {group.kind !== 'linked' && <span className="tag">{t(`worktree.${group.kind}`)}</span>}
        </>
      ),
      // 노드 나무 뒤에 자식 묶음이 온다(그래프의 묶음 나무와 같다).
      children: [...nodeItems(inGroup(views, group), null, props.agents, rename, note(at)), ...childGroups(at)],
    });
    const childGroups = (parent: number | null): Item[] => groups.flatMap((g, i) => (parents[i] === parent ? [groupItem(g, i)] : []));
    return childGroups(null);
  };
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
    children: rootGroups(repo),
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
    const shown = Math.min(depth, MAX_VISUAL_DEPTH);
    const head = (
      <>
        {Array.from({ length: shown }, (_, level) => (
          <span key={level} className="guide" style={{ '--level': level } as CSSProperties} aria-hidden="true" />
        ))}
        {branch ? <Chevron open={open} /> : <span className="leaf" />}
        {item.row}
      </>
    );
    return (
      <li key={item.key} role="treeitem" aria-expanded={branch ? open : undefined} aria-selected={item.node !== null ? item.node === props.selected : undefined}>
        {item.node !== null && item.node === renaming ? (
          <div className="tree-row" style={{ '--depth': shown } as CSSProperties} aria-current={item.node === props.selected ? 'true' : undefined}>
            {head}
          </div>
        ) : (
          <button
            className="tree-row"
            style={{ '--depth': shown } as CSSProperties}
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
