import { Bookmark, ChevronDown, ChevronsUpDown, CornerLeftUp, EllipsisVertical, GitBranch, GitBranchPlus, ListX, Pencil, Plus, Trash2 } from 'lucide-react';
import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type KeyboardEvent, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { removeNode, removeRepo, renameWorktree } from './api';
import { AgentMark } from './AgentMark';
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
import { NameEdit, NameField, returnFocus, type Rename } from './NameEdit';
import { NewNodePopover } from './NewNodePopover';
import { shownPath, ShortPlaceTag } from './place';
import { NewWorktreePopover } from './NewWorktreePopover';
import { modelText, stateClass, stateLabel, stateText, Unread } from './NodeCard';
import { NoticeChips } from './notice';
import { MenuSheet, Sheet, useLongPress, usePhone } from './Phone';
import { useDismiss } from './popover';
import { groupParents, groupsOf, inGroup, type Group } from './repos';

type Action = { label: string; icon: ReactNode; run: (row: HTMLElement) => void; disabled?: boolean; pressed?: boolean };
type Item = { key: string; title: string; row: ReactNode; node: NodeId | null; box?: string; children: Item[]; actions: Action[]; rename?: Action; remove?: Action; show?: () => void };

type Target = { repo: RepoId } & ({ kind: 'newWorktree' } | { kind: 'newNode'; path: string } | { kind: 'deleteWorktree'; path: string });
type Opened = { target: Target; key: string; top: number; left: number };

const POP_GAP = 8;

const foundKey = (f: Found) => (f.kind === 'node' ? `node:${f.node}` : `group:${f.path}`);

function keepFound(items: Item[], hits: ReadonlySet<string>): Item[] {
  return items.flatMap((item) => {
    const children = keepFound(item.children, hits);
    return hits.has(item.key) || children.length > 0 ? [{ ...item, children }] : [];
  });
}

const rowActions = (item: Item): Action[] => [...item.actions, ...(item.rename ? [item.rename] : []), ...(item.remove ? [item.remove] : [])];

const MAX_VISUAL_DEPTH = 8;


function Chevron({ open }: { open: boolean }) {
  return <ChevronDown className={open ? 'chev' : 'chev closed'} />;
}

function FromIcon({ title }: { title: string }) {
  return (
    <CornerLeftUp className="from" role="img" aria-label={title}>
      <title>{title}</title>
    </CornerLeftUp>
  );
}

type ParentNote = (v: NodeView) => string | null;
type NodeActions = (view: NodeView) => Pick<Item, 'actions' | 'rename' | 'remove'>;

function RowPop(props: { top: number; left: number; onClose: () => void; children: ReactNode }) {
  const box = useDismiss<HTMLDivElement>(props.onClose);
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

function StateTag({ view }: { view: NodeView }) {
  if (view.waits.length > 0) return <NoticeChips view={view} />;
  if (view.state.kind !== 'exited' && view.state.kind !== 'setupFailed') return null;
  return <span className={`state-tag ${view.state.kind}`}>{stateLabel(view.state)}</span>;
}

function nodeItems(views: NodeView[], parent: NodeId | null, agents: AgentView[], rename: (key: string) => Rename, note: ParentNote, acts: NodeActions): Item[] {
  return views
    .filter((v) => (parent === null ? !views.some((p) => p.id === v.parent) : v.parent === parent))
    .sort((a, b) => a.id - b.id)
    .map((v) => {
      const from = note(v);
      const label = agentLabel(agents, v.agent);
      return {
        key: `node:${v.id}`,
        title: v.name,
        node: v.id,
        row: (
          <span className="node-row">
            <span className="node-line">
              <span className={`dot ${stateClass(v)}`} aria-label={stateText(v)} />
              <NameEdit id={v.id} name={v.name} rename={rename(`node:${v.id}`)} />
              {from !== null && <FromIcon title={from} />}
              {v.unread > 0 && <span className="spacer" />}
              {v.unread > 0 && <Unread n={v.unread}>{v.unread}</Unread>}
              <StateTag view={v} />
            </span>
            <span className="node-line">
              <AgentMark agent={v.agent} label={label} />
              <span className="sub" translate="no">
                {modelText(v) || label}
              </span>
            </span>
          </span>
        ),
        children: nodeItems(views, v.id, agents, rename, note, acts),
        ...acts(v),
      };
    });
}

export function Tree(props: {
  repos: RepoView[];
  current: RepoId | null;
  onRepo: (id: RepoId) => void;
  onClear: () => void;
  onAdded: (repo: RepoView) => void;
  views: ReadonlyMap<NodeId, NodeView>;
  agents: AgentView[];
  onRescanAgents: () => void;
  selected: NodeId | null;
  onSelect: (id: NodeId) => void;
  chosenBox: string | null;
  onChooseBox: (key: string) => void;
  settings: SettingsView;
  accounts: AccountView[] | null;
  onRepoChanged: (repo: RepoView) => void;
  onRepoRemoved: (id: RepoId) => void;
  find: { query: string; found: readonly Found[]; at: number; onAt: (at: number) => void } | null;
}) {
  const { t } = useTranslation();
  const show = useShowNodes();
  const showGroups = useShowGroups();
  const showAll = useShowAll();
  const phone = usePhone();
  const [menu, setMenu] = useState<string | null>(null);
  const longPress = useLongPress((row) => setMenu(row.dataset.key ?? null));
  const [adding, setAdding] = useState(false);
  const [closed, setClosed] = useState<ReadonlySet<string>>(new Set());
  const list = useRef<HTMLUListElement>(null);
  const hits = props.find?.found.map(foundKey) ?? [];
  const isOpen = (key: string) => props.find !== null || !closed.has(key);
  const repoKey = (id: RepoId) => `repo:${id}`;
  const [active, setActive] = useState('');
  const [renaming, setRenaming] = useState<string | null>(null);
  const [opened, setOpened] = useState<Opened | null>(null);
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
    remove: { label: t('node.remove'), icon: <Trash2 />, run: () => removeNode(id).catch(refuse(`node:${id}`)) },
    };
  };
  const rename = (key: string): Rename => ({
    editing: renaming === key,
    open: () => setRenaming(key),
    close: () => {
      setRenaming((k) => (k === key ? null : k));
      returnFocus(() => rowOf(key));
    },
    refused: (message) => setRefusal({ key, text: message }),
  });
  const toggle = (key: string, open: boolean) =>
    setClosed((prev) => {
      const next = new Set(prev);
      if (open) next.delete(key);
      else next.add(key);
      return next;
    });

  const views = [...props.views.values()];
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
      const branch = group.kind === 'linked' ? group.branch : null;
      return {
        key: groupKey,
        title: group.label,
        node: null,
        box: group.path,
        actions: [{ label: t('graph.newNode'), icon: <Plus />, run: (row) => openPop(row, { kind: 'newNode', repo: repo.id, path: group.path }) }],
        rename: branch === null ? undefined : { label: t('rename.action'), icon: <Pencil />, run: () => setRenaming(groupKey) },
        remove: group.kind === 'linked' ? { label: t('deleteWorktree.action'), icon: <Trash2 />, run: (row) => openPop(row, { kind: 'deleteWorktree', repo: repo.id, path: group.path }) } : undefined,
        show: () => {
          if (repo.id !== props.current) props.onRepo(repo.id);
          props.onChooseBox(group.path);
          showGroups([group.path]);
        },
        row: (
          <>
            <GitBranch className="icon" />
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
        children: [...nodeItems(inGroup(views, group), null, props.agents, rename, note(at), nodeActions), ...childGroups(at)],
      };
    };
    const childGroups = (parent: number | null): Item[] => groups.flatMap((g, i) => (parents[i] === parent ? [groupItem(g, i)] : []));
    return childGroups(null);
  };
  const all: Item[] = props.repos.map((repo) => ({
    key: repoKey(repo.id),
    title: repo.name,
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
    remove: { label: t('tree.removeRepo'), icon: <ListX />, run: () => removeRepo(repo.id).then(() => props.onRepoRemoved(repo.id), refuse(repoKey(repo.id))) },
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
    const actions = rowActions(item);
    return (
      <li key={item.key} role="treeitem" aria-expanded={branch ? open : undefined} aria-selected={item.node !== null ? item.node === props.selected : undefined}>
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
              {...(phone ? longPress : {})}
              tabIndex={item.key === tabStop ? 0 : -1}
              onFocus={() => setActive(item.key)}
              aria-current={(item.node !== null && item.node === props.selected) || (item.box !== undefined && item.box === props.chosenBox) ? 'true' : undefined}
              onClick={(e) => {
                if (hit >= 0) props.find?.onAt(hit);
                if (item.show && !phone && !(e.target as Element).closest('.chev')) return item.show();
                if (item.node === null) return toggle(item.key, !open);
                show([item.node]);
                props.onSelect(item.node);
              }}
            >
              {head}
            </button>
          )}
          {!editing && phone && (
            <button type="button" className="row-menu" aria-label={t('tree.rowMenu', { name: item.title })} aria-haspopup="dialog" onClick={() => setMenu(item.key)}>
              <EllipsisVertical aria-hidden="true" />
            </button>
          )}
          {!editing && !phone && (
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

  const onKeyDown = (e: KeyboardEvent<HTMLUListElement>) => {
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
  const currentHit = props.find && hits[props.find.at];
  useEffect(() => {
    list.current?.querySelector('[data-found="current"]')?.scrollIntoView({ block: 'nearest' });
  }, [currentHit]);

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
          onOpen={props.onRescanAgents}
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
  const popLabel: Record<Target['kind'], string> = { newWorktree: t('repo.newWorktree'), newNode: t('graph.newNode'), deleteWorktree: t('deleteWorktree.action') };
  const addRepo = (
    <AddRepoPopover
      onClose={() => setAdding(false)}
      onAdded={(repo) => {
        setAdding(false);
        props.onAdded(repo);
      }}
    />
  );
  const menuItem = menu === null ? undefined : visible.get(menu);

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
      {opened &&
        content &&
        (phone ? (
          <Sheet label={popLabel[opened.target.kind]} onClose={closePop}>
            {content}
          </Sheet>
        ) : (
          <RowPop top={opened.top} left={opened.left} onClose={closePop}>
            {content}
          </RowPop>
        ))}
      {adding &&
        (phone ? (
          <Sheet label={t('tree.addRepo')} onClose={() => setAdding(false)}>
            {addRepo}
          </Sheet>
        ) : (
          addRepo
        ))}
      {menuItem && (
        <MenuSheet
          title={menuItem.title}
          onClose={() => setMenu(null)}
          items={rowActions(menuItem).map((action) => ({
            label: action.label,
            icon: action.icon,
            disabled: action.disabled,
            danger: action === menuItem.remove,
            confirm: action === menuItem.remove && menuItem.node !== null,
            run: () => {
              setRefusal(null);
              action.run(rowOf(menuItem.key)!);
            },
          }))}
        />
      )}
    </>
  );
}
