// 사이드바 트리(docs/mockups/graph-panels.html 1번): 저장소 > 워크트리 묶음 > 노드 > 자식 노드.
// 그래프와 같은 노드 목록을 그린다. 트리만의 상태는 접힌 줄뿐이다.
import { useState, type CSSProperties, type KeyboardEvent, type ReactNode } from 'react';
import { agentLabel } from './catalog';
import type { AgentView } from './gen/AgentView';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { RepoView } from './gen/RepoView';
import { MAIN_GROUP, useShowNodes } from './Graph';

type Item = { key: string; row: ReactNode; node: NodeId | null; children: Item[] };

const REPO_KEY = 'repo';

function Chevron({ open }: { open: boolean }) {
  return (
    <svg className={open ? 'chev' : 'chev closed'} viewBox="0 0 24 24" aria-hidden="true">
      <path d="M6 9l6 6 6-6" />
    </svg>
  );
}

function nodeItems(views: NodeView[], parent: NodeId | null, agents: AgentView[]): Item[] {
  return views
    .filter((v) => v.parent === parent)
    .sort((a, b) => a.id - b.id)
    .map((v) => ({
      key: `node:${v.id}`,
      node: v.id,
      row: (
        <>
          <span className={`dot ${v.state.kind}`} aria-label={v.state.kind} />
          <span className="name">{v.name}</span>
          <span className="sub">{agentLabel(agents, v.agent)}</span>
          {v.unread > 0 && (
            <>
              <span className="spacer" />
              <span className="unread" aria-label={`읽지 않은 메시지 ${v.unread}개`}>
                {v.unread}
              </span>
            </>
          )}
        </>
      ),
      children: nodeItems(views, v.id, agents),
    }));
}

export function Tree(props: { repo: RepoView; views: ReadonlyMap<NodeId, NodeView>; agents: AgentView[]; selected: NodeId | null; onSelect: (id: NodeId) => void }) {
  const show = useShowNodes();
  const [closed, setClosed] = useState<ReadonlySet<string>>(new Set());
  // Tab으로 들어올 때 포커스를 받는 줄(roving tabindex).
  const [active, setActive] = useState(REPO_KEY);
  const toggle = (key: string, open: boolean) =>
    setClosed((prev) => {
      const next = new Set(prev);
      if (open) next.delete(key);
      else next.add(key);
      return next;
    });

  const root: Item = {
    key: REPO_KEY,
    node: null,
    row: (
      <>
        <span className="name">{props.repo.name}</span>
        <span className="sub" translate="no">
          {props.repo.path}
        </span>
      </>
    ),
    children: [
      {
        key: `group:${MAIN_GROUP}`,
        node: null,
        row: (
          <>
            <span className="branch" translate="no">
              {MAIN_GROUP}
            </span>
            <span className="sub">원래 체크아웃</span>
          </>
        ),
        children: nodeItems([...props.views.values()], null, props.agents),
      },
    ],
  };

  const visible = new Set<string>();
  const walk = (item: Item) => {
    visible.add(item.key);
    if (!closed.has(item.key)) item.children.forEach(walk);
  };
  walk(root);
  const tabStop = visible.has(active) ? active : REPO_KEY;

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
          aria-current={item.node !== null && item.node === props.selected ? 'true' : undefined}
          onClick={() => {
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
        <h2>저장소</h2>
        <span className="spacer" />
        <button className="btn ghost sm" aria-label="모두 펼치기" title="모두 펼치기" onClick={() => setClosed(new Set())}>
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M7 15l5 5 5-5M7 9l5-5 5 5" />
          </svg>
        </button>
      </div>
      <ul className="tree" role="tree" aria-label="노드 트리" onKeyDown={onKeyDown}>
        {render(root, 0, null)}
      </ul>
    </>
  );
}
