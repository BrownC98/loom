// 사이드바 트리(docs/mockups/graph-panels.html 1번): 저장소 > 워크트리 묶음 > 노드 > 자식 노드.
// 그래프와 같은 노드 목록을 그린다. 트리만의 상태는 접힌 줄, 이름 편집, 줄 단추로 연 팝오버와 그 거절 글이다.
import { Bookmark, ChevronDown, ChevronsUpDown, CornerLeftUp, GitBranch, GitBranchPlus, ListX, Pencil, Plus, Trash2 } from 'lucide-react';
import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type KeyboardEvent, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { removeNode, removeRepo, renameWorktree } from './api';
import { agentLabel, recentAgent } from './catalog';
import type { AccountView } from './gen/AccountView';
import type { AgentView } from './gen/AgentView';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { RepoId } from './gen/RepoId';
import type { RepoView } from './gen/RepoView';
import type { SettingsView } from './gen/SettingsView';
import { AddRepoPopover } from './AddRepoPopover';
import { bookmarkToggle } from './BookmarkButton';
import { DeleteWorktreePopover } from './DeleteWorktreePopover';
import type { Found } from './findSpots';
import { useShowAll, useShowGroups, useShowNodes } from './Graph';
import i18next from './i18n';
import { NameEdit, NameField, returnFocus, type Rename } from './NameEdit';
import { NewNodePopover } from './NewNodePopover';
import { shownPath, ShortPlaceTag } from './place';
import { NewWorktreePopover } from './NewWorktreePopover';
import { stateClass, Unread } from './NodeCard';
import { useOutsideClose } from './popover';
import { groupParents, groupsOf, inGroup, type Group } from './repos';

/** 줄 끝 단추 하나(docs/mockups/sidebar-crud.html 2번). `run`은 누른 줄의 단추를 받는다. 팝오버를 그 줄 옆에 띄운다. `pressed`가 있으면 켜고 끄는 단추다(북마크). */
type Action = { label: string; icon: ReactNode; run: (row: HTMLElement) => void; disabled?: boolean; pressed?: boolean };
/** `rename`은 줄의 이름 바꾸기 동작이다. F2 키도 부른다. `remove`는 줄의 지우기 동작이다. 단추 끝에 오고 Delete 키도 부른다. `show`는 줄을 누르면 그래프를 옮긴다(저장소 줄, 워크트리 줄). */
type Item = { key: string; row: ReactNode; node: NodeId | null; /** 이 줄이 그래프에서 가리키는 상자(`flowId`). */ box?: string; children: Item[]; actions: Action[]; rename?: Action; remove?: Action; show?: () => void };

/** 줄 단추로 연 팝오버. 그릴 때 저장소와 묶음을 다시 찾는다. 그 사이에 사라졌으면 그리지 않는다. */
type Target = { repo: RepoId } & ({ kind: 'newWorktree' } | { kind: 'newNode'; path: string } | { kind: 'deleteWorktree'; path: string });
/** `key`는 누른 줄이다. 닫으면 포커스가 그 줄로 돌아간다. */
type Opened = { target: Target; key: string; top: number; left: number };

/** 팝오버와 사이드바 오른쪽 경계 사이 틈, 창 아래 끝과 남기는 틈. */
const POP_GAP = 8;

/** 찾기 일치의 줄 키(`Item.key`). */
const foundKey = (f: Found) => (f.kind === 'node' ? `node:${f.node}` : `group:${f.path}`);

/** 찾는 동안 남기는 줄: 일치 줄과 그 위 줄(목업 sidebar-find.html ③). */
function keepFound(items: Item[], hits: ReadonlySet<string>): Item[] {
  return items.flatMap((item) => {
    const children = keepFound(item.children, hits);
    return hits.has(item.key) || children.length > 0 ? [{ ...item, children }] : [];
  });
}

/** 들여쓰기 깊이의 화면 상한. 더 깊은 줄은 같은 자리에 두어 이름이 보일 폭을 지킨다(styles.css `--step` 12px × 8 = 96px). */
const MAX_VISUAL_DEPTH = 8;


function Chevron({ open }: { open: boolean }) {
  return <ChevronDown className={open ? 'chev' : 'chev closed'} />;
}

/** 부모가 나무의 부모 묶음 밖에 있는 노드의 표시(corner-left-up). `title`이 부모 노드와 묶음을 말한다. */
function FromIcon({ title }: { title: string }) {
  return (
    <CornerLeftUp className="from" role="img" aria-label={title}>
      <title>{title}</title>
    </CornerLeftUp>
  );
}

/** 노드 줄의 `부모: … · …` 문구를 돌려준다. 문구가 없으면 표시도 없다. */
type ParentNote = (v: NodeView) => string | null;
type NodeActions = (view: NodeView) => Pick<Item, 'actions' | 'rename' | 'remove'>;

/** 줄 옆 팝오버 자리. 창 아래로 넘치면 위로 올린다. 내용이 늦게 와 커져도(새 노드의 프로필 목록) 다시 맞춘다. 밖을 누르면 닫는다(새 노드 팝오버는 스스로 닫지 않는다). */
function RowPop(props: { top: number; left: number; onClose: () => void; children: ReactNode }) {
  const box = useOutsideClose<HTMLDivElement>(props.onClose);
  const [top, setTop] = useState(props.top);
  useLayoutEffect(() => {
    const el = box.current!;
    const fit = () => setTop(Math.max(POP_GAP, Math.min(props.top, innerHeight - el.offsetHeight - POP_GAP)));
    fit();
    const observer = new ResizeObserver(fit);
    observer.observe(el);
    return () => observer.disconnect();
  }, [props.top]);
  return (
    <div ref={box} className="row-pop" style={{ top, left: props.left }}>
      {props.children}
    </div>
  );
}

/** `parent`가 없으면 묶음의 뿌리: 부모가 없거나 부모가 다른 묶음에 있는 노드다. */
function nodeItems(views: NodeView[], parent: NodeId | null, agents: AgentView[], rename: (key: string) => Rename, note: ParentNote, acts: NodeActions): Item[] {
  return views
    .filter((v) => (parent === null ? !views.some((p) => p.id === v.parent) : v.parent === parent))
    .sort((a, b) => a.id - b.id)
    .map((v) => {
      const from = note(v);
      return {
        key: `node:${v.id}`,
        node: v.id,
        row: (
          <>
            <span className={`dot ${stateClass(v)}`} aria-label={stateClass(v)} />
            <NameEdit id={v.id} name={v.name} rename={rename(`node:${v.id}`)} />
            {from !== null && <FromIcon title={from} />}
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
        children: nodeItems(views, v.id, agents, rename, note, acts),
        ...acts(v),
      };
    });
}

export function Tree(props: {
  repos: RepoView[];
  /** 현재 저장소. 저장소 하나 범위면 그래프가 그리는 저장소이고, 커밋 그래프 칸이 보이는 저장소다. */
  current: RepoId | null;
  onRepo: (id: RepoId) => void;
  /** 「저장소」 제목을 누르면 모든 저장소를 한 화면에 담고 고른 노드·상자·저장소 포커스를 지운다. */
  onClear: () => void;
  onAdded: (repo: RepoView) => void;
  views: ReadonlyMap<NodeId, NodeView>;
  agents: AgentView[];
  selected: NodeId | null;
  onSelect: (id: NodeId) => void;
  /** 묶음·저장소 줄을 누르면 그래프의 그 상자를 고른다(`flowId`). */
  chosenBox: string | null;
  /** 묶음·저장소 줄을 누르면 하는 일. 폰은 그래프가 없어 접고 편다. */
  rowTap: 'graph' | 'fold';
  onChooseBox: (key: string) => void;
  settings: SettingsView;
  accounts: AccountView[] | null;
  /** 워크트리를 만들거나 지운 뒤 저장소의 새 모양. */
  onRepoChanged: (repo: RepoView) => void;
  /** 저장소를 목록에서 뺐다. 그래프가 그 저장소를 그리면 App이 다른 저장소로 바꾼다. */
  onRepoRemoved: (id: RepoId) => void;
  /** 찾는 중이면 일치 목록(App의 `findSpots`), 현재 일치 번호, 줄을 눌러 현재 일치를 바꾸는 동작. 그동안 트리는 일치 줄과 그 위 줄만 보이고 모두 편다. */
  find: { query: string; found: readonly Found[]; at: number; onAt: (at: number) => void } | null;
}) {
  const { t } = useTranslation();
  const show = useShowNodes();
  const showGroups = useShowGroups();
  const showAll = useShowAll();
  const [adding, setAdding] = useState(false);
  const [closed, setClosed] = useState<ReadonlySet<string>>(new Set());
  const list = useRef<HTMLUListElement>(null);
  const hits = props.find?.found.map(foundKey) ?? [];
  const isOpen = (key: string) => props.find !== null || !closed.has(key);
  const repoKey = (id: RepoId) => `repo:${id}`;
  // Tab으로 들어올 때 포커스를 받는 줄(roving tabindex).
  const [active, setActive] = useState('');
  // 이름을 고치는 줄(노드, 워크트리). 그 줄은 단추 대신 div다. 단추 안에는 입력창을 두지 못한다.
  const [renaming, setRenaming] = useState<string | null>(null);
  const [opened, setOpened] = useState<Opened | null>(null);
  // 줄 동작의 거절(저장소에 노드가 남음 등). 그 줄 아래에 보인다.
  const [refusal, setRefusal] = useState<{ key: string; text: string } | null>(null);
  const rowOf = (key: string) => document.querySelector<HTMLElement>(`button.tree-row[data-key="${CSS.escape(key)}"]`);
  const openPop = (row: HTMLElement, target: Target) => {
    const rect = row.getBoundingClientRect();
    const side = row.closest('.side')?.getBoundingClientRect().right ?? rect.right;
    setOpened({ target, key: row.dataset.key!, top: rect.top, left: side + POP_GAP });
  };
  const closePop = () => {
    const key = opened?.key;
    setOpened(null);
    if (key) returnFocus(() => rowOf(key));
  };
  const refuse = (key: string) => (err: Error) => setRefusal({ key, text: err.message });
  const nodeActions: NodeActions = (view) => {
    const id = view.id;
    const bookmark = bookmarkToggle(view);
    return {
      actions: [{ label: bookmark.label, icon: <Bookmark />, pressed: bookmark.pressed, run: () => bookmark.run().catch(refuse(`node:${id}`)) }],
    rename: { label: t('rename.action'), icon: <Pencil />, run: () => setRenaming(`node:${id}`) },
    // 카드와 같이 확인 없이 바로 지운다(docs/design.md 「노드 제거와 입양」).
    remove: { label: t('node.remove'), icon: <Trash2 />, run: () => removeNode(id).catch(refuse(`node:${id}`)) },
    };
  };
  const rename = (key: string): Rename => ({
    editing: renaming === key,
    open: () => setRenaming(key),
    close: () => {
      setRenaming(null);
      returnFocus(() => rowOf(key));
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
    const groupItem = (group: Group, at: number): Item => {
      const groupKey = `group:${group.path}`;
      // 원래 체크아웃과 분리된 HEAD는 이름을 바꾸지 않는다. 데몬도 거절한다.
      const branch = group.kind === 'linked' ? group.branch : null;
      return {
        key: groupKey,
        node: null,
        box: group.path,
        actions: [{ label: t('graph.newNode'), icon: <Plus />, run: (row) => openPop(row, { kind: 'newNode', repo: repo.id, path: group.path }) }],
        rename: branch === null ? undefined : { label: t('rename.action'), icon: <Pencil />, run: () => setRenaming(groupKey) },
        // 원래 체크아웃은 지우지 않는다.
        remove: group.kind === 'linked' ? { label: t('deleteWorktree.action'), icon: <Trash2 />, run: (row) => openPop(row, { kind: 'deleteWorktree', repo: repo.id, path: group.path }) } : undefined,
        // 다른 저장소의 묶음이면 그래프를 그 저장소로 바꾼다. 그 묶음을 아직 그리지 않았어도 `showGroup`이 잰 뒤 옮긴다.
        show: () => {
          if (repo.id !== props.current) props.onRepo(repo.id);
          props.onChooseBox(group.path);
          showGroups([group.path]);
        },
        row: (
          <>
            <GitBranch className="icon" />
            {/* 긴 브랜치는 한 줄에서 줄임표로 자른다. 툴팁이 전체 이름과 경로를 보인다. 들여쓰기가 워크트리 층을 보이므로 원래 체크아웃에만 꼬리표를 단다(Orca 사이드바). */}
            {branch === null ? (
              <span className="branch" translate="no" title={`${group.label}\n${group.path}`}>
                {group.label}
              </span>
            ) : (
              <NameField name={branch} label={t('repo.branch')} detail={`${group.label}\n${group.path}`} rename={rename(groupKey)} save={(name) => renameWorktree(repo.id, group.path, name).then(props.onRepoChanged)} />
            )}
            {group.kind !== 'linked' && <span className="tag">{t(`worktree.${group.kind}`)}</span>}
          </>
        ),
        // 노드 나무 뒤에 자식 묶음이 온다(그래프의 묶음 나무와 같다).
        children: [...nodeItems(inGroup(views, group), null, props.agents, rename, note(at), nodeActions), ...childGroups(at)],
      };
    };
    const childGroups = (parent: number | null): Item[] => groups.flatMap((g, i) => (parents[i] === parent ? [groupItem(g, i)] : []));
    return childGroups(null);
  };
  const all: Item[] = props.repos.map((repo) => ({
    key: repoKey(repo.id),
    node: null,
    box: `repo:${repo.id}`,
    actions: [
      {
        label: repo.hasCommits ? t('repo.newWorktree') : `${t('repo.newWorktree')}: ${t('repo.noCommit')}`,
        icon: <GitBranchPlus />,
        disabled: !repo.hasCommits,
        run: (row) => openPop(row, { kind: 'newWorktree', repo: repo.id }),
      },
    ],
    // 폴더는 그대로 둔다. 노드가 남았으면 데몬이 거절한다.
    remove: { label: t('tree.removeRepo'), icon: <ListX />, run: () => removeRepo(repo.id).then(() => props.onRepoRemoved(repo.id), refuse(repoKey(repo.id))) },
    // 그 저장소를 현재 저장소로 두고 그 묶음이 모두 보이게 옮긴다(목업 repo-scope.html 1번). 저장소 하나 범위면 그래프가 그 저장소로 바뀐다.
    show: () => {
      props.onRepo(repo.id);
      props.onChooseBox(`repo:${repo.id}`);
      showGroups(groupsOf(repo, views).map((g) => g.path));
    },
    row: (
      <>
        <span className="name">{repo.name}</span>
        <ShortPlaceTag wsl={repo.wsl} />
        <span className="sub" translate="no" title={repo.path}>
          {shownPath(repo)}
        </span>
      </>
    ),
    children: rootGroups(repo),
  }));
  const roots = props.find === null ? all : keepFound(all, new Set(hits));
  // 보이는 줄. Delete 키가 줄의 지우기 동작을 찾는다.
  const visible = new Map<string, Item>();
  const walk = (item: Item) => {
    visible.set(item.key, item);
    if (isOpen(item.key)) item.children.forEach(walk);
  };
  roots.forEach(walk);
  const tabStop = visible.has(active) ? active : (roots[0]?.key ?? '');

  const render = (item: Item, depth: number, parentKey: string | null): ReactNode => {
    const open = isOpen(item.key);
    const hit = hits.indexOf(item.key);
    const branch = item.children.length > 0;
    const shown = Math.min(depth, MAX_VISUAL_DEPTH);
    const head = (
      <>
        {branch ? <Chevron open={open} /> : <span className="leaf" />}
        {item.row}
      </>
    );
    const editing = item.key === renaming;
    const actions = [...item.actions, ...(item.rename ? [item.rename] : []), ...(item.remove ? [item.remove] : [])];
    return (
      <li key={item.key} role="treeitem" aria-expanded={branch ? open : undefined} aria-selected={item.node !== null ? item.node === props.selected : undefined}>
        {/* 줄 단추와 동작 단추는 형제다. 단추 안에는 단추를 두지 못한다. */}
        <div className="tree-line">
          {editing ? (
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
              data-found={props.find === null ? undefined : hit < 0 ? 'context' : hit === props.find.at ? 'current' : 'hit'}
              tabIndex={item.key === tabStop ? 0 : -1}
              onFocus={() => setActive(item.key)}
              aria-current={(item.node !== null && item.node === props.selected) || (item.box !== undefined && item.box === props.chosenBox) ? 'true' : undefined}
              onClick={(e) => {
                if (hit >= 0) props.find?.onAt(hit);
                // 저장소 줄과 워크트리 줄은 그래프를 그 자리로 옮긴다. 화살표를 누르면 접고 편다.
                if (item.show && props.rowTap === 'graph' && !(e.target as Element).closest('.chev')) return item.show();
                if (item.node === null) return toggle(item.key, !open);
                show([item.node]);
                props.onSelect(item.node);
              }}
            >
              {head}
            </button>
          )}
          {!editing && (
            <span className="row-acts">
              {actions.map((action) => (
                <button
                  key={action.label}
                  type="button"
                  className={action === item.remove ? 'act remove' : 'act'}
                  aria-label={action.label}
                  title={action.label}
                  disabled={action.disabled}
                  aria-pressed={action.pressed}
                  onClick={(e) => {
                    setRefusal(null);
                    action.run(e.currentTarget.closest('.tree-line')!.querySelector<HTMLElement>('.tree-row')!);
                  }}
                >
                  {action.icon}
                </button>
              ))}
            </span>
          )}
        </div>
        {refusal?.key === item.key && (
          <p className="row-refusal" role="alert" style={{ '--depth': shown } as CSSProperties}>
            {refusal.text}
          </p>
        )}
        {branch && open && <ul role="group">{item.children.map((child) => render(child, depth + 1, item.key))}</ul>}
      </li>
    );
  };

  // 트리 키보드: ↑↓ 줄 옮기기, → 펴기, ← 접기 또는 부모 줄로, F2 이름 바꾸기(노드, 워크트리), Delete 줄 지우기, Enter·Space는 단추 기본 동작(고르기).
  // 줄의 동작 단추에 포커스가 있어도 그 줄의 키로 본다. 이름 입력창(줄이 단추가 아님)에서는 아무것도 하지 않는다.
  const onKeyDown = (e: KeyboardEvent<HTMLUListElement>) => {
    // 보이는 줄 단추, 위에서부터.
    const rows = [...e.currentTarget.querySelectorAll<HTMLButtonElement>('button.tree-row')];
    const current = (e.target as HTMLElement).closest('.tree-line')?.querySelector<HTMLButtonElement>('button.tree-row');
    if (!current) return;
    const at = rows.indexOf(current);
    const focus = (row: HTMLButtonElement | undefined) => row?.focus();
    if (e.key === 'ArrowDown') focus(rows[at + 1]);
    else if (e.key === 'ArrowUp') focus(rows[Math.max(0, at - 1)]);
    else if (e.key === 'ArrowRight' && current?.dataset.open === 'false') toggle(current.dataset.key!, true);
    else if (e.key === 'ArrowLeft' && current?.dataset.open === 'true') toggle(current.dataset.key!, false);
    else if (e.key === 'ArrowLeft' && current?.dataset.parent) focus(rows.find((r) => r.dataset.key === current.dataset.parent));
    else if (e.key === 'F2' && visible.get(current.dataset.key!)?.rename) setRenaming(current.dataset.key!);
    else if (e.key === 'Delete' && visible.get(current.dataset.key!)?.remove) {
      setRefusal(null);
      visible.get(current.dataset.key!)!.remove!.run(current);
    } else return;
    e.preventDefault();
  };

  // 일치 줄의 맞는 글자를 칠한다. DOM을 바꾸지 않는 CSS Custom Highlight API다(styles.css `::highlight(find)`).
  useEffect(() => {
    const needle = props.find?.query.trim().toLocaleLowerCase();
    if (!needle || !list.current || !('highlights' in CSS)) return;
    const ranges = [...list.current.querySelectorAll('.tree-row:is([data-found="hit"], [data-found="current"]) :is(.name, .branch)')].flatMap((el) => {
      const text = el.firstChild;
      const at = text instanceof Text ? text.data.toLocaleLowerCase().indexOf(needle) : -1;
      if (!(text instanceof Text) || at < 0 || at + needle.length > text.length) return [];
      const range = new Range();
      range.setStart(text, at);
      range.setEnd(text, at + needle.length);
      return [range];
    });
    CSS.highlights.set('find', new Highlight(...ranges));
    return () => void CSS.highlights.delete('find');
  });
  // 현재 일치 줄이 트리 칸 밖이면 보이게 굴린다.
  const currentHit = props.find && hits[props.find.at];
  useEffect(() => {
    list.current?.querySelector('[data-found="current"]')?.scrollIntoView({ block: 'nearest' });
  }, [currentHit]);

  /** 열린 줄 팝오버. 내용은 그래프의 팝오버 그대로다. */
  const popContent = ({ target }: Opened): ReactNode => {
    const repo = props.repos.find((r) => r.id === target.repo);
    if (!repo) return null;
    if (target.kind === 'newWorktree')
      return (
        <NewWorktreePopover
          repo={repo}
          workspaceDir={props.settings.workspaceDir ?? ''}
          onClose={closePop}
          onCreated={(next) => {
            closePop();
            props.onRepoChanged(next);
          }}
        />
      );
    const group = groupsOf(repo, views).find((g) => g.path === target.path);
    if (!group) return null;
    if (target.kind === 'newNode')
      return (
        <NewNodePopover
          branch={group.label}
          place={{ repo: repo.id, worktree: group.path }}
          distro={repo.wsl?.distro ?? null}
          agents={props.agents}
          recent={recentAgent(views)}
          settings={props.settings}
          accounts={props.accounts}
          onCreated={(id) => {
            setOpened(null);
            props.onSelect(id);
          }}
          onCancel={closePop}
        />
      );
    return (
      <DeleteWorktreePopover
        repo={repo.id}
        group={group}
        views={inGroup(views, group)}
        onDeleted={(next) => {
          setOpened(null);
          props.onRepoChanged(next);
        }}
        onClose={closePop}
      />
    );
  };
  const content = opened && popContent(opened);

  return (
    <>
      <div className="side-head">
        <h2>
          <button type="button" className="side-title" title={t('tree.showAll')}
            onClick={() => {
              showAll();
              props.onClear();
            }}
          >
            {t('tree.repos')}
          </button>
        </h2>
        <span className="spacer" />
        <button className="btn ghost sm" aria-label={t('tree.addRepo')} title={t('tree.addRepo')} aria-expanded={adding} onClick={() => setAdding(true)}>
          <Plus />
        </button>
        <button className="btn ghost sm" aria-label={t('tree.expandAll')} title={t('tree.expandAll')} onClick={() => setClosed(new Set())}>
          <ChevronsUpDown />
        </button>
      </div>
      <ul ref={list} className="tree" role="tree" aria-label={t('tree.label')} onKeyDown={onKeyDown}>
        {roots.map((root) => render(root, 0, null))}
      </ul>
      {props.find !== null && roots.length === 0 && <p className="tree-none">{t('find.noMatch')}</p>}
      {opened && content && (
        <RowPop top={opened.top} left={opened.left} onClose={closePop}>
          {content}
        </RowPop>
      )}
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
