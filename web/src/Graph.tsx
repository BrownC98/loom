import { GitBranchPlus, MapMinus, MapPlus, Maximize2, Minimize2, Plus, Trash2, Workflow } from 'lucide-react';
import { Fragment, memo, useCallback, useContext, useEffect, useImperativeHandle, useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode, type Ref, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import { applyNodeChanges, Handle, MarkerType, MiniMap, NodeResizeControl, Panel, Position, ResizeControlVariant, ReactFlow, useReactFlow, useStore, useStoreApi, useUpdateNodeInternals, ViewportPortal, type Edge, type EdgeMarker, type FinalConnectionState, type Node, type NodeChange, type NodeProps, type ResizeParams, type XYPosition } from '@xyflow/react';
import { adoptNode, clearSizes, clearSpots, getSizes, getSpots, nodePty, putArranged, putSizes, putSpots, removeNode } from './api';
import { agentLabel, recentAgent } from './catalog';
import type { AccountView } from './gen/AccountView';
import type { AgentView } from './gen/AgentView';
import type { CardView } from './gen/CardView';
import type { DefaultCardView } from './gen/DefaultCardView';
import { cardViewOf } from './cardView';
import type { Toward } from './gen/Toward';
import type { NodeId } from './gen/NodeId';
import type { RepoId } from './gen/RepoId';
import type { NodeView } from './gen/NodeView';
import type { Placed } from './gen/Placed';
import type { RepoView } from './gen/RepoView';
import type { WslPlace } from './gen/WslPlace';
import type { SettingsView } from './gen/SettingsView';
import type { Spot } from './gen/Spot';
import { DeleteWorktreePopover } from './DeleteWorktreePopover';
import { BendEdge } from './BendEdge';
import { HopEdge, hopLabelSize, type HopEdgeType } from './HopEdge';
import { hopColor, hopParts, siblingLanes, type HopEndName, type HopPair } from './hopLanes';
import { arrangeNested, bendCenter, edgeSides, followCard, FREE_GAP, GROUP_GAP, labelHeading, overlaps, push, samePoint, settle, SIDES, stepAt, swap, TREE_SIDES, unpile, type ArrangeDirection, type Box, type NestGroup, type Side, type Sibling, type Size, type Swap } from './layout';
import { NewNodePopover } from './NewNodePopover';
import { useOutsidePress } from './popover';
import { PlaceTag, shownPath } from './place';
import { NewWorktreePopover } from './NewWorktreePopover';
import { BookmarkButton } from './BookmarkButton';
import { NodeMenu } from './NodeMenu';
import { isControl, NodeCard, SimpleCard } from './NodeCard';
import { minimapClass } from './notice';
import { attachCards, groupParents, groupsOf, inGroup, splitChips, type Group, type GroupKind } from './repos';
import { attachDragPan, attachPinchZoom, type View } from './pinchZoom';
import { realignTerminals, SizingTerminal, XtermFont, type Draw } from './Terminal';
import type { Direction } from './keys';
import { flowId, MOVE_MS } from './camera';
import { SLOP_PX } from './touchTerminal';

const OWN_TOUCH = 'input, textarea, [contenteditable], .react-flow__handle, .react-flow__resize-control, [data-selected] .term';
const GROUP_POSITION: XYPosition = { x: 16, y: 64 };
const REPO_Z = -1;
const RAISED_EDGE_Z = 1;
const EDGE_TYPES = { hop: HopEdge, bend: BendEdge };
const ARROW = 12;
const arrowHead = (color: string): EdgeMarker => ({ type: MarkerType.ArrowClosed, color, width: ARROW, height: ARROW, markerUnits: 'userSpaceOnUse' });
const DRAGGING_Z = 2000;
const EMPTY_GROUP: Size = { width: 320, height: 100 };
const CARD_X = 20;
const CARD_TOP = 60;
const STEP_BEND = ARROW;
const CARD_SIZE: Record<CardView, Size> = {
  standard: { width: 600, height: 466 },
  simple: { width: 260, height: 134 },
};
const MIN_CARD: Size = { width: 360, height: 200 };
const POSITION: Record<Side, Position> = { top: Position.Top, right: Position.Right, bottom: Position.Bottom, left: Position.Left };
const POPOVER_GAP = 6;
const POPOVER_EDGE = 8;
const CHIP_ROW = 43;
const REPO_HEAD = 46;
const REPO_SIDE = 16;
const REPO_INSET = { top: REPO_HEAD + REPO_SIDE, side: REPO_SIDE };
const MIN_ZOOM = 0.1;
const MAX_ZOOM = 3.5;

type Align = 'start' | 'end';

function PopAt(props: { at: XYPosition; align: Align; opener?: RefObject<HTMLElement | null>; onClose: () => void; children: ReactNode }) {
  const box = useRef<HTMLDivElement>(null);
  useOutsidePress(box, props.onClose, props.opener);
  const [shift, setShift] = useState(0);
  const transform = useStore((s) => s.transform);
  useLayoutEffect(() => {
    const pop = box.current?.firstElementChild;
    const pane = box.current?.closest('.react-flow');
    if (!pop || !pane) return;
    const zoom = transform[2];
    const rect = pop.getBoundingClientRect();
    const room = pane.getBoundingClientRect();
    const left = rect.left - shift * zoom;
    const right = rect.right - shift * zoom;
    const next = Math.max((room.left + POPOVER_EDGE - left) / zoom, Math.min(0, (room.right - POPOVER_EDGE - right) / zoom));
    if (Math.abs(next - shift) > 0.5) setShift(next);
  }, [props.at.x, props.at.y, transform, shift]);
  return (
    <div ref={box} className={`pop-${props.align}`} style={{ position: 'absolute', transform: `translate(${props.at.x + shift}px, ${props.at.y}px)` }}>
      {props.children}
    </div>
  );
}

function UnzoomVar() {
  const store = useStoreApi();
  const zoom = useStore((s) => s.transform[2]);
  useLayoutEffect(() => {
    store.getState().domNode?.style.setProperty('--unzoom', String(1 / zoom));
  }, [store, zoom]);
  return null;
}

function ZoomLevel(props: { folded: boolean; onFold: (folded: boolean) => void }) {
  const { t } = useTranslation();
  const flow = useReactFlow();
  const zoom = useStore((s) => s.transform[2]);
  const [draft, setDraft] = useState<string | null>(null);
  const apply = () => {
    const percent = Number(draft);
    if (draft !== null && draft.trim() !== '' && Number.isFinite(percent) && percent > 0) flow.zoomTo(percent / 100, { duration: MOVE_MS });
    setDraft(null);
  };
  return (
    <Panel position="bottom-right" className="zoom-level">
      <IconButton label={t(props.folded ? 'graph.minimapUnfold' : 'graph.minimapFold')} title={t(props.folded ? 'graph.minimapUnfold' : 'graph.minimapFold')} onClick={() => props.onFold(!props.folded)}>
        {props.folded ? <MapPlus /> : <MapMinus />}
      </IconButton>
      {draft === null ? (
        <span title={t('graph.zoomEdit')} onDoubleClick={() => setDraft(String(Math.round(zoom * 100)))}>
          {Math.round(zoom * 100)}%
        </span>
      ) : (
        <input
          aria-label={t('graph.zoomEdit')}
          inputMode="numeric"
          autoFocus
          onFocus={(e) => e.currentTarget.select()}
          value={draft}
          onChange={(e) => setDraft(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') apply();
            if (e.key === 'Escape') setDraft(null);
          }}
          onBlur={() => setDraft(null)}
        />
      )}
    </Panel>
  );
}

function IconButton(props: { label: string; title: string; children: ReactNode; onClick: () => void }) {
  return (
    <button type="button" className="btn ghost sm icon nodrag" aria-label={props.label} title={props.title} onClick={props.onClick}>
      {props.children}
    </button>
  );
}

type GroupData = {
  branch: string;
  kind: GroupKind;
  empty: boolean;
  creating: boolean;
  deleting: boolean;
  onOpen: (button: HTMLElement) => void;
  onOpenAt: (point: XYPosition) => void;
  onDelete?: (button: HTMLElement) => void;
  onArrange: () => void;
  chips: readonly Chip[];
  onOpenChip: (path: string, button: HTMLElement) => void;
  onDeleteChip: (path: string, button: HTMLElement) => void;
  onChipRow: (height: number) => void;
};
type Chip = { path: string; label: string; className: string | undefined; creating: boolean; deleting: boolean };
type CardBase = { view: NodeView; defaultView: DefaultCardView; agentLabel: string; onRefusal: (text: string) => void; selected: boolean; focusAsk: number; port: Side; onSelect: () => void; onNavigate: (dir: Direction) => void; onRemove: () => void };
type CardData = CardBase & { parentName: string | null; ownsPty: boolean; onExpand: () => void; sized: boolean; onResizeStart: () => void; onResize: (step: Resized) => void; onResized: (step: Resized) => void; onResetSize: () => void };
type Resized = { shift: XYPosition; size: Size };
type RepoData = { name: string; path: string; wsl: WslPlace | null; hasCommits: boolean; creating: boolean; onArrange: () => void; onNewWorktree: (button: HTMLElement) => void };
type GroupNode = Node<GroupData, 'worktree'>;
type RepoNode = Node<RepoData, 'repo'>;
type CardNode = Node<CardData, 'card'>;
type SimpleNode = Node<CardBase, 'simple'>;
type FlowNode = RepoNode | GroupNode | CardNode | SimpleNode;
type Fn = (...args: never[]) => unknown;

function Group({ data }: NodeProps<GroupNode>) {
  const { t } = useTranslation();
  const chipRow = useRef<HTMLDivElement>(null);
  const hasChips = data.chips.length > 0;
  useLayoutEffect(() => {
    const el = chipRow.current;
    if (!el) return;
    const sizeObserver = new ResizeObserver(() => data.onChipRow(el.offsetHeight));
    sizeObserver.observe(el);
    return () => sizeObserver.disconnect();
  }, [hasChips, data.onChipRow]);
  return (
    <section
      className="group"
      aria-label={t('graph.group', { branch: data.branch })}
      onMouseDown={(e) => {
        if (e.button !== 0 || (e.target as Element).closest('.group-head, .nodrag')) return;
        e.currentTarget.closest('.react-flow')?.querySelector('.react-flow__pane')?.dispatchEvent(new MouseEvent('mousedown', e.nativeEvent));
      }}
      onContextMenu={(e) => {
        if ((e.target as Element).closest('button, .wt-chip')) return;
        e.preventDefault();
        data.onOpenAt({ x: e.clientX, y: e.clientY });
      }}
    >
      <div className="group-head">
        <span className="branch" translate="no" title={data.branch}>
          {data.branch}
        </span>
        {data.kind !== 'linked' && <span className="kind">{t(`worktree.${data.kind}`)}</span>}
        <span className="spacer" />
        <IconButton label={t('graph.arrangeGroup')} title={t('graph.arrangeGroup')} onClick={data.onArrange}>
          <Workflow />
        </IconButton>
        <button className="btn ghost sm nodrag" aria-expanded={data.creating} onClick={(e) => data.onOpen(e.currentTarget)}>
          <Plus />
          {t('graph.newNode')}
        </button>
        {data.onDelete && (
          <button className="btn ghost sm nodrag" aria-expanded={data.deleting} onClick={(e) => data.onDelete?.(e.currentTarget)}>
            {t('deleteWorktree.action')}
          </button>
        )}
      </div>
      {hasChips && (
        <div ref={chipRow} className="wt-chips">
          <span className="label">{t('graph.chips')}</span>
          {data.chips.map((chip) => (
            <span key={chip.path} className={['wt-chip', chip.className].filter(Boolean).join(' ')} aria-label={t('graph.group', { branch: chip.label })}>
              <span className="branch" translate="no" title={chip.path}>
                {chip.label}
              </span>
              <button type="button" className="btn ghost sm icon nodrag" aria-label={t('graph.newNode')} title={t('graph.newNode')} aria-expanded={chip.creating} onClick={(e) => data.onOpenChip(chip.path, e.currentTarget)}>
                <Plus />
              </button>
              <button type="button" className="btn ghost sm icon nodrag" aria-label={t('deleteWorktree.action')} title={t('deleteWorktree.action')} aria-expanded={chip.deleting} onClick={(e) => data.onDeleteChip(chip.path, e.currentTarget)}>
                <Trash2 />
              </button>
            </span>
          ))}
        </div>
      )}
      {data.empty && <p className="group-empty">{t('graph.empty')}</p>}
    </section>
  );
}

function Repo({ data }: NodeProps<RepoNode>) {
  const { t } = useTranslation();
  return (
    <section className="repo-box" aria-label={data.name}>
      <div className="repo-head">
        <span className="name">{data.name}</span>
        <PlaceTag wsl={data.wsl} />
        <span className="sub" translate="no" title={data.path}>
          {shownPath(data)}
        </span>
        <span className="spacer" />
        <IconButton label={t('graph.arrangeRepo')} title={t('graph.arrangeRepo')} onClick={data.onArrange}>
          <Workflow />
        </IconButton>
        <button
          className="btn ghost sm nodrag"
          disabled={!data.hasCommits}
          title={data.hasCommits ? undefined : `${t('repo.newWorktree')}: ${t('repo.noCommit')}`}
          aria-expanded={data.creating}
          onClick={(e) => data.onNewWorktree(e.currentTarget)}
        >
          <GitBranchPlus />
          {t('repo.newWorktree')}
        </button>
      </div>
    </section>
  );
}

const RESIZE_EDGES = [
  { position: 'left', variant: ResizeControlVariant.Line },
  { position: 'right', variant: ResizeControlVariant.Line },
  { position: 'top', variant: ResizeControlVariant.Line },
  { position: 'bottom', variant: ResizeControlVariant.Line },
  { position: 'top-left', variant: ResizeControlVariant.Handle },
  { position: 'top-right', variant: ResizeControlVariant.Handle },
  { position: 'bottom-left', variant: ResizeControlVariant.Handle },
  { position: 'bottom-right', variant: ResizeControlVariant.Handle },
] as const;

const READABLE_TEXT_PX = 6;
const HIDDEN_KEEP_MS = 3000;

function useDraw(id: string, fontSize: number): Draw | null {
  return useStore((s) => {
    const node = s.nodeLookup.get(id);
    if (!node) return null;
    const [x, y, zoom] = s.transform;
    const left = node.internals.positionAbsolute.x * zoom + x;
    const top = node.internals.positionAbsolute.y * zoom + y;
    const right = left + (node.measured.width ?? 0) * zoom;
    const bottom = top + (node.measured.height ?? 0) * zoom;
    if (right <= -s.width || left >= 2 * s.width || bottom <= -s.height || top >= 2 * s.height) return null;
    const shown = right > 0 && left < s.width && bottom > 0 && top < s.height;
    if (!shown) return 'hidden';
    return fontSize * zoom > READABLE_TEXT_PX ? 'live' : 'slow';
  });
}

function Card({ id, data }: NodeProps<CardNode>) {
  const { t } = useTranslation();
  const draw = useDraw(id, useContext(XtermFont).size);
  const [gone, setGone] = useState(false);
  useEffect(() => {
    setGone(false);
    if (draw !== 'hidden') return;
    const timer = setTimeout(() => setGone(true), HIDDEN_KEEP_MS);
    return () => clearTimeout(timer);
  }, [draw]);
  const termBox = useRef<HTMLDivElement>(null);
  const [hold, setHold] = useState<Size | null>(null);
  const start = useRef<XYPosition>({ x: 0, y: 0 });
  const step = (p: ResizeParams): Resized => ({ shift: { x: p.x - start.current.x, y: p.y - start.current.y }, size: { width: p.width, height: p.height } });
  return (
    <>
      <NodeCard
        view={data.view}
        agentLabel={data.agentLabel}
        parentName={data.parentName}
        selected={data.selected}
        onSelect={data.onSelect}
        onNavigate={data.onNavigate}
        onDoubleClickHead={data.onExpand}
        headAction={
          <>
            <BookmarkButton view={data.view} />
            <IconButton label={t('node.expand')} title={t('node.expandTitle')} onClick={data.onExpand}>
              <Maximize2 />
            </IconButton>
            <NodeMenu view={data.view} defaultView={data.defaultView} onResetSize={data.sized ? data.onResetSize : undefined} onRemove={data.onRemove} onRefusal={data.onRefusal} />
          </>
        }
      >
        <div ref={termBox} className="term-hold" style={hold ?? undefined}>
          {data.ownsPty && draw !== null && !gone && <SizingTerminal src={nodePty(data.view.id)} node={data.view.id} draw={data.selected && draw !== 'hidden' ? 'live' : draw} focused={data.selected} focusAsk={data.focusAsk} onFocus={data.onSelect} onNavigate={data.onNavigate} />}
        </div>
      </NodeCard>
      {RESIZE_EDGES.map((edge) => (
        <NodeResizeControl
          key={edge.position}
          position={edge.position}
          variant={edge.variant}
          minWidth={MIN_CARD.width}
          minHeight={MIN_CARD.height}
          onResizeStart={(_, p) => {
            start.current = { x: p.x, y: p.y };
            const el = termBox.current;
            if (el) setHold({ width: el.offsetWidth, height: el.offsetHeight });
            data.onResizeStart();
          }}
          onResize={(_, p) => data.onResize(step(p))}
          onResizeEnd={(_, p) => {
            setHold(null);
            data.onResized(step(p));
          }}
        />
      ))}
      <Ports port={data.port} />
    </>
  );
}

function Simple({ data }: NodeProps<SimpleNode>) {
  return (
    <>
      <SimpleCard view={data.view} agentLabel={data.agentLabel} selected={data.selected} onSelect={data.onSelect} onNavigate={data.onNavigate} actions={
          <>
            <BookmarkButton view={data.view} />
            <NodeMenu view={data.view} defaultView={data.defaultView} onRemove={data.onRemove} onRefusal={data.onRefusal} />
          </>
        }
      />
      <Ports port={data.port} />
    </>
  );
}

function Ports(props: { port: Side }) {
  return SIDES.map((side) => (
    <Fragment key={side}>
      <Handle type="target" id={side} position={POSITION[side]} isConnectable={false} />
      <Handle type="source" id={side} position={POSITION[side]} isConnectable={side === props.port} className={side === props.port ? 'port' : undefined} />
    </Fragment>
  ));
}

function sameData(a: { data: object }, b: { data: object }): boolean {
  const [x, y] = [a.data as Record<string, unknown>, b.data as Record<string, unknown>];
  const keys = Object.keys(x);
  return keys.length === Object.keys(y).length && keys.every((k) => x[k] === y[k]);
}

const nodeTypes = { repo: memo(Repo, sameData), worktree: memo(Group, sameData), card: memo(Card, sameData), simple: memo(Simple, sameData) };

function neighbor(positions: Map<NodeId, XYPosition>, from: NodeId, dir: Direction): NodeId | null {
  const origin = positions.get(from);
  if (!origin) return null;
  let best: NodeId | null = null;
  let bestScore = Infinity;
  for (const [id, p] of positions) {
    const dx = p.x - origin.x;
    const dy = p.y - origin.y;
    const [ahead, aside] = dir === 'left' ? [-dx, dy] : dir === 'right' ? [dx, dy] : dir === 'up' ? [-dy, dx] : [dy, dx];
    if (ahead <= 0) continue;
    const score = ahead + 2 * Math.abs(aside);
    if (score < bestScore) [best, bestScore] = [id, score];
  }
  return best;
}

function parentName(views: ReadonlyMap<NodeId, NodeView>, view: NodeView): string | null {
  return view.parent === null ? null : (views.get(view.parent)?.name ?? null);
}

function cover(contents: readonly Box[], inset: { top: number; side: number }): Box {
  const x = Math.min(...contents.map((b) => b.at.x)) - inset.side;
  const y = Math.min(...contents.map((b) => b.at.y)) - inset.top;
  return {
    at: { x, y },
    size: { width: Math.max(...contents.map((b) => b.at.x + b.size.width)) + inset.side - x, height: Math.max(...contents.map((b) => b.at.y + b.size.height)) + inset.side - y },
  };
}

type Level = { boxes: Sibling[]; lead: ReadonlyMap<string, XYPosition>; parent: string | null };
type Step = { key: string; level: Level; mover: Box };
type Draft = { spots?: ReadonlyMap<string, XYPosition>; size?: { id: NodeId; size: Size }; clearing?: readonly RepoId[] };

export type Lit = { keys: ReadonlySet<string>; current: string | null };

export type Pulse = Readonly<{ hue: number; last: Toward; counts: Readonly<Record<Toward, number>> }>;

export type Hop = Readonly<{ source: NodeId; target: NodeId; hue: number; last: HopEndName; counts: Readonly<Record<HopEndName, number>> }>;

export type GraphHandle = { neighbor: (from: NodeId, dir: Direction) => NodeId | null; arrange: () => void; hasSizes: () => boolean; resetSizes: () => void };

export function Graph(props: {
  repos: readonly RepoView[];
  onRepo: (repo: RepoView) => void;
  views: ReadonlyMap<NodeId, NodeView>;
  pulses: ReadonlyMap<NodeId, Pulse>;
  hops: ReadonlyMap<string, Hop>;
  agents: AgentView[];
  onRescanAgents: () => void;
  settings: SettingsView;
  viewVersion: number;
  onMinimapFold: (folded: boolean) => void;
  accounts: AccountView[] | null;
  selected: NodeId | null;
  focusAsk: number;
  onSelect: (id: NodeId | null) => void;
  onClear: () => void;
  chosenBox: string | null;
  onChooseBox: (key: string | null) => void;
  highlight: Lit | null;
  onClearHighlight: () => void;
  direction: ArrangeDirection;
  ref?: Ref<GraphHandle>;
}) {
  const { t } = useTranslation();
  const { highlight } = props;
  const lit = (...keys: string[]) => (highlight === null ? undefined : keys.every((key) => highlight.keys.has(key)) ? 'hl' : 'dim');
  const litGroup = (path: string, inside: string[]) => (highlight !== null && !highlight.keys.has(path) && inside.some((key) => highlight.keys.has(key)) ? undefined : lit(path));
  const classes = (key: string, base: string | undefined) => [base, highlight?.current === key ? 'cur' : undefined].filter(Boolean).join(' ') || undefined;
  const { selected, onSelect: setSelected, chosenBox, onChooseBox: setChosenBox } = props;
  const flow = useReactFlow();
  const [expanded, setExpanded] = useState<NodeId | null>(null);
  const zoomed = expanded !== null && selected !== null && props.views.has(selected) ? selected : expanded;
  const [creating, setCreating] = useState<string | null>(null);
  const [anchor, setAnchor] = useState<{ at: XYPosition; align: Align } | null>(null);
  const opener = useRef<HTMLElement | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [chipRows, setChipRows] = useState<ReadonlyMap<string, number>>(new Map());
  const [branching, setBranching] = useState<{ key: string; anchor: XYPosition } | null>(null);
  const [refusal, setRefusal] = useState('');
  const minimapFolded = props.settings.minimapFolded;
  const [flowState, setFlowState] = useState<ReadonlyMap<string, FlowNode>>(new Map());
  const [spots, setSpots] = useState<ReadonlyMap<string, Placed>>(new Map());
  const repoOf = (spot: Spot) => (spot.kind === 'node' ? props.views.get(spot.node)?.place.repo : spot.repo);
  const [arrangedRepos, setArrangedRepos] = useState<ReadonlySet<RepoId>>(new Set());
  const [arranging, setArranging] = useState<{ scope: { kind: 'shown' } | { kind: 'repo'; id: RepoId }; after: Promise<unknown> } | null>(null);
  const arrangingRepos = arranging === null ? [] : arranging.scope.kind === 'repo' ? [arranging.scope.id] : props.repos.map((r) => r.id);
  const [pending, setPending] = useState<ReadonlyMap<number, Draft & { after: number }>>(new Map());
  const drafts = [...pending.values()];
  const clearing = new Set([...drafts.flatMap((d) => d.clearing ?? []), ...arrangingRepos]);
  const draftAt = new Map(drafts.flatMap((d) => [...(d.spots ?? [])]));
  const draftSize = new Map(drafts.flatMap((d): [NodeId, Size][] => (d.size ? [[d.size.id, d.size.size]] : [])));
  const arrangedNow = (repo: RepoId) => arrangedRepos.has(repo) && !clearing.has(repo);
  const { alwaysArrange } = props.settings;
  const keepsArranged = (repo: RepoId) => alwaysArrange || arrangedNow(repo);
  const savedSpot = (key: string) => {
    const saved = spots.get(key);
    const repo = saved && repoOf(saved.spot);
    return repo !== undefined && clearing.has(repo) ? undefined : saved;
  };
  const spotAt = (key: string) => draftAt.get(key) ?? savedSpot(key)?.at;
  const [moving, setMoving] = useState<ReadonlyMap<string, XYPosition>>(new Map());
  const swaps = useRef<{ key: string; swap: Swap } | null>(null);
  const [sizes, setSizes] = useState<ReadonlyMap<NodeId, Size>>(new Map());
  const savedSize = (id: NodeId) => draftSize.get(id) ?? sizes.get(id);
  const [resizing, setResizing] = useState<{ id: NodeId; size: Size } | null>(null);
  const reads = useRef(0);
  const readLayout = () => {
    const asked = ++reads.current;
    Promise.all([getSpots(), getSizes()]).then(
      ([layout, all]) => {
        if (asked !== reads.current) return;
        setSpots(new Map(layout.placed.map((p) => [flowId(p.spot), p])));
        setArrangedRepos(new Set(layout.arranged));
        setSizes(new Map(all.map((s) => [s.node, { width: s.width, height: s.height }])));
        setPending((prev) => new Map([...prev].filter(([, d]) => d.after > asked)));
      },
      (err: Error) => {
        setRefusal(err.message);
        if (asked === reads.current) setPending((prev) => new Map([...prev].filter(([, d]) => d.after > asked)));
      },
    );
    return asked;
  };
  useEffect(() => void readLayout(), [props.viewVersion]);
  const commits = useRef(0);
  const commit = (write: Promise<unknown>, draft: Draft) => {
    const id = ++commits.current;
    setPending((prev) => new Map(prev).set(id, { ...draft, after: Infinity }));
    void write
      .catch((err: Error) => setRefusal(err.message))
      .then(() => {
        const after = readLayout();
        setPending((prev) => {
          const d = prev.get(id);
          return d ? new Map(prev).set(id, { ...d, after }) : prev;
        });
      });
  };
  const { direction } = props;
  const outers = useRef<ReadonlyMap<string, string>>(new Map());
  const placedAt = (key: string): XYPosition | undefined => moving.get(key) ?? (stale.has(key) ? undefined : spotAt(key));

  const [order, setOrder] = useState<ReadonlyMap<string, number>>(new Map());
  const holdOrder = () => setOrder((prev) => new Map([...prev, ...onScreen()]));
  const rank = (key: string) => {
    const saved = spotAt(key);
    return saved ? (direction === 'down' || direction === 'up' ? saved.x : saved.y) : (order.get(key) ?? Infinity);
  };
  const byRank = <T,>(items: readonly T[], key: (item: T) => string, tie: (a: T, b: T) => number) => [...items].sort((a, b) => rank(key(a)) - rank(key(b)) || tie(a, b));
  const views = byRank([...props.views.values()], (v) => String(v.id), (a, b) => a.id - b.id);
  const viewOf = (id: NodeId): CardView => {
    const view = props.views.get(id);
    return view ? cardViewOf(view, props.settings.cardView) : 'standard';
  };
  const standard = (id: NodeId) => viewOf(id) === 'standard';
  useEffect(() => {
    if (zoomed !== null && !standard(zoomed)) setExpanded(null);
  });
  const chosenSize = (id: NodeId): Size | undefined => (!standard(id) ? undefined : resizing?.id === id ? resizing.size : savedSize(id));
  const size = (id: NodeId): Size => {
    const measured = flowState.get(String(id))?.measured;
    return measured?.width && measured.height ? { width: measured.width, height: measured.height } : (chosenSize(id) ?? CARD_SIZE[viewOf(id)]);
  };
  const levels = new Map<string, Level>();
  const spotOf = new Map<string, Spot>();
  const branchOf = new Map<string, string>();
  const share = (level: Level) => {
    for (const b of level.boxes) levels.set(b.key, level);
  };
  type Laid = { group: Group; members: NodeView[]; local: Map<string, XYPosition>; auto: Map<string, XYPosition>; lead: XYPosition; size: Size; kids: Laid[] };
  const emptySize = (path: string): Size => {
    const measured = flowState.get(path)?.measured;
    return { width: measured?.width ?? EMPTY_GROUP.width, height: measured?.height ?? EMPTY_GROUP.height };
  };
  const framed = props.repos.length > 1;
  const vertical = direction === 'down' || direction === 'up';
  const chipsIn = new Map<string, Group[]>();
  const chipsOf = (path: string) => chipsIn.get(path) ?? [];
  const insetOf = (path: string) => ({ top: CARD_TOP + (chipsIn.has(path) ? (chipRows.get(path) ?? CHIP_ROW) : 0), side: CARD_X });
  const nests = props.repos.map((repo) => {
    const repoViews = views.filter((v) => v.place.repo === repo.id);
    const { boxes, chips } = splitChips(groupsOf(repo, repoViews), repoViews);
    const repoGroups = byRank(boxes, (g) => g.path, () => 0);
    const host = repoGroups.find((g) => g.kind !== 'linked');
    if (host && chips.length > 0) chipsIn.set(host.path, chips);
    const parents = groupParents(repoGroups, repoViews);
    const cards = attachCards(repoGroups, repoViews, parents);
    const memberOf = repoGroups.map((g) => inGroup(repoViews, g));
    const nest = repoGroups.map((g, i): NestGroup => ({ parent: cards[i] === null ? null : parents[i], card: cards[i], members: memberOf[i].map((v) => v.id), empty: emptySize(g.path) }));
    return { repo, repoGroups, memberOf, nest };
  });
  const outerNow = new Map(nests.flatMap(({ repo, repoGroups, nest }) => repoGroups.map((g, i): [string, string] => [g.path, nest[i].parent === null ? `repo:${repo.id}` : repoGroups[nest[i].parent!].path])));
  const stale = new Set([...outerNow].flatMap(([path, outer]) => (outers.current.has(path) && outers.current.get(path) !== outer ? [path] : [])));
  const unpiled = (at: Map<string, XYPosition>, items: readonly { key: string; size: Size }[]) =>
    moving.size > 0 ? at : unpile(items.map((it) => ({ key: it.key, at: at.get(it.key)!, size: it.size })), FREE_GAP);
  const repos = nests.map(({ repo, repoGroups, memberOf, nest }) => {
    const placedIn = keepsArranged(repo.id) ? (key: string) => moving.get(key) : placedAt;
    const tree = arrangeNested(nest, (id) => props.views.get(id)?.parent ?? null, size, direction, { top: CARD_TOP, side: CARD_X }, (kid) => rank('card' in kid ? String(kid.card) : repoGroups[kid.group].path));
    const lay = (i: number): Laid => {
      const group = repoGroups[i];
      const members = memberOf[i];
      const kids = nest.flatMap((n, k) => (n.parent === i ? [{ laid: lay(k), auto: tree.boxes[k].at, card: n.card }] : []));
      const base = tree.boxes[i].at;
      const rel = (p: XYPosition) => ({ x: p.x - base.x, y: p.y - base.y });
      const items = [
        ...members.map((v) => ({ key: String(v.id), size: size(v.id), lead: { x: 0, y: 0 }, auto: rel(tree.cards.get(v.id)!) })),
        ...kids.map((k) => ({ key: k.laid.group.path, size: k.laid.size, lead: k.laid.lead, auto: rel(k.auto) })),
      ];
      const local = unpiled(settle(items, placedIn, FREE_GAP), items);
      for (const v of members) {
        spotOf.set(String(v.id), { kind: 'node', node: v.id });
        branchOf.set(String(v.id), v.parent !== null && members.some((m) => m.id === v.parent) ? String(v.parent) : 'root');
      }
      for (const k of kids) {
        spotOf.set(k.laid.group.path, { kind: 'worktree', repo: repo.id, path: k.laid.group.path });
        branchOf.set(k.laid.group.path, String(k.card));
      }
      share({ boxes: items.map((it) => ({ key: it.key, at: local.get(it.key)!, size: it.size })), lead: new Map(kids.map((k) => [k.laid.group.path, k.laid.lead])), parent: group.path });
      const box = items.length > 0 ? cover(items.map((it) => ({ at: local.get(it.key)!, size: it.size })), insetOf(group.path)) : { at: { x: 0, y: 0 }, size: nest[i].empty };
      const auto = new Map(items.map((it): [string, XYPosition] => [it.key, { x: it.auto.x - it.lead.x, y: it.auto.y - it.lead.y }]));
      return { group, members, local, auto, lead: box.at, size: box.size, kids: kids.map((k) => k.laid) };
    };
    const top = nest.flatMap((n, i) => (n.parent === null ? [{ laid: lay(i), auto: tree.boxes[i].at }] : []));
    const tops = top.map(({ laid: g, auto }) => ({ key: g.group.path, size: g.size, lead: g.lead, auto }));
    const at = unpiled(settle(tops, placedIn, FREE_GAP), tops);
    for (const { laid: g } of top) spotOf.set(g.group.path, { kind: 'worktree', repo: repo.id, path: g.group.path });
    const key = `repo:${repo.id}`;
    const laid = top.map((t) => t.laid);
    share({ boxes: laid.map((g) => ({ key: g.group.path, at: at.get(g.group.path)!, size: g.size })), lead: new Map(laid.map((g) => [g.group.path, g.lead])), parent: framed ? key : null });
    const box = cover(laid.map((g) => ({ at: at.get(g.group.path)!, size: g.size })), REPO_INSET);
    const all = (g: Laid): Laid[] => [g, ...g.kids.flatMap(all)];
    const arranged = new Map([...laid.flatMap(all).flatMap((g) => [...g.auto]), ...top.map(({ laid: g, auto }): [string, XYPosition] => [g.group.path, { x: auto.x - g.lead.x, y: auto.y - g.lead.y }])]);
    return { repo, key, laid: laid.map((g) => ({ ...g, at: at.get(g.group.path)! })), lead: box.at, size: box.size, arranged };
  });
  const reversed = direction === 'up' || direction === 'left';
  const alongOf = (s: Size) => (vertical ? s.height : s.width);
  const deepest = Math.max(0, ...repos.map((r) => alongOf(r.size)));
  const repoRow = byRank(repos, (r) => r.key, () => 0);
  const repoAuto = new Map(
    repoRow.map((r, i): [string, XYPosition] => {
      const across = repoRow.slice(0, i).reduce((sum, q) => sum + (vertical ? q.size.width : q.size.height) + GROUP_GAP, 0);
      const along = reversed ? deepest - alongOf(r.size) : 0;
      return [r.key, { x: GROUP_POSITION.x + (vertical ? across : along), y: GROUP_POSITION.y + (vertical ? along : across) }];
    }),
  );
  const repoItems = repos.map((r) => ({ key: r.key, size: r.size, lead: r.lead, auto: repoAuto.get(r.key)! }));
  const repoAt = unpiled(settle(repoItems, alwaysArrange ? (key: string) => moving.get(key) : placedAt, FREE_GAP), repoItems);
  for (const r of repos) spotOf.set(r.key, { kind: 'repo', repo: r.repo.id });
  if (framed) share({ boxes: repos.map((r) => ({ key: r.key, at: repoAt.get(r.key)!, size: r.size })), lead: new Map(repos.map((r) => [r.key, r.lead])), parent: null });
  const repoOrigin = (r: (typeof repos)[number]) => (framed ? { x: repoAt.get(r.key)!.x - r.lead.x, y: repoAt.get(r.key)!.y - r.lead.y } : GROUP_POSITION);
  const positions = new Map<NodeId, XYPosition>();
  const flatten = (g: Laid, origin: XYPosition, repo: RepoId, outer: { path: string; at: XYPosition } | null): (Laid & { repo: RepoId; origin: XYPosition; outer: { path: string; at: XYPosition } | null })[] => {
    for (const v of g.members) {
      const p = g.local.get(String(v.id))!;
      positions.set(v.id, { x: origin.x + p.x - g.lead.x, y: origin.y + p.y - g.lead.y });
    }
    const kids = g.kids.flatMap((k) => {
      const p = g.local.get(k.group.path)!;
      const at = { x: p.x - g.lead.x, y: p.y - g.lead.y };
      return flatten(k, { x: origin.x + at.x, y: origin.y + at.y }, repo, { path: g.group.path, at });
    });
    return [{ ...g, repo, origin, outer }, ...kids];
  };
  const groups = repos.flatMap((r) => {
    const o = repoOrigin(r);
    return r.laid.flatMap((g) => flatten(g, { x: o.x + g.at.x, y: o.y + g.at.y }, r.repo.id, null));
  });
  const onScreen = (): Map<string, number> => {
    const cross = (p: XYPosition) => (direction === 'down' || direction === 'up' ? p.x : p.y);
    return new Map([...[...positions].map(([id, p]): [string, number] => [String(id), cross(p)]), ...groups.map((g): [string, number] => [g.group.path, cross(g.origin)])]);
  };
  const navigate = (from: NodeId) => (dir: Direction) => {
    const to = neighbor(positions, from, dir);
    if (to !== null) setSelected(to);
  };
  useImperativeHandle(props.ref, () => ({
    neighbor: (from, dir) => neighbor(positions, from, dir),
    arrange: () => {
      const shown = props.repos.map((r) => r.id);
      setMoving(new Map());
      holdOrder();
      setArranging({ scope: { kind: 'shown' }, after: Promise.all(shown.map(clearSpots)) });
    },
    hasSizes: () => views.some((v) => standard(v.id) && savedSize(v.id) !== undefined),
    resetSizes: () => resetSizes(views.filter((v) => standard(v.id) && savedSize(v.id) !== undefined).map((v) => v.id)),
  }));
  const resetSizes = (ids: NodeId[]) => {
    if (ids.length === 0) return;
    commit(clearSizes(ids), {});
  };
  const resizeFrom = useRef<{ key: string; at: XYPosition } | null>(null);
  const resizeTo = (key: string, step: Resized, done: ReadonlyMap<string, XYPosition>) => {
    const level = levels.get(key)!;
    const from = resizeFrom.current!.at;
    const at = { x: from.x + step.shift.x, y: from.y + step.shift.y };
    return spread(key, level, { at, size: step.size }, new Map([...pinned(level, done), [key, originOf(level, key, at)]]));
  };
  const carried = new Set<string>(resizing ? [String(resizing.id)] : []);
  const dragged = new Set([...flowState.values()].filter((n) => n.dragging).map((n) => n.id));
  for (const id of [...dragged, ...carried]) {
    for (let outer = levels.get(id)?.parent ?? null; outer !== null; outer = levels.get(outer)?.parent ?? null) carried.add(outer);
  }
  const lastFocus = useRef<{ id: NodeId; at: XYPosition } | null>(null);
  const cameraMoving = useRef(false);
  const focusAt = selected === null ? undefined : positions.get(selected);
  const last = lastFocus.current;
  const handsOff = dragged.size === 0 && resizing === null;
  const follow = focusAt && last?.id === selected && !samePoint(last.at, focusAt) && handsOff && !cameraMoving.current ? { from: last.at, to: focusAt } : null;
  if (selected !== null && handsOff) carried.add(String(selected));
  useLayoutEffect(() => {
    lastFocus.current = selected !== null && focusAt ? { id: selected, at: focusAt } : null;
    if (follow) void flow.setViewport(followCard(follow.from, follow.to, flow.getViewport()));
  });
  const pane = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = pane.current;
    if (!el) return;
    const view = { get: flow.getViewport, set: (v: View) => void flow.setViewport(v) };
    const pinch = attachPinchZoom(el, view, { min: MIN_ZOOM, max: MAX_ZOOM });
    const pan = attachDragPan(el, view, (target) => target.closest('.react-flow__node') !== null && !target.closest(OWN_TOUCH), SLOP_PX);
    return () => {
      pinch();
      pan();
    };
  }, [flow]);
  useEffect(() => {
    const el = pane.current;
    if (!el) return;
    let left = el.getBoundingClientRect().left;
    const keep = new ResizeObserver(() => {
      const now = el.getBoundingClientRect().left;
      if (now === left) return;
      const { x, y, zoom } = flow.getViewport();
      void flow.setViewport({ x: x - (now - left), y, zoom });
      left = now;
    });
    keep.observe(el);
    return () => keep.disconnect();
  }, [flow]);
  const insideDragged = (id: string) => {
    for (let outer = levels.get(id)?.parent ?? null; outer !== null; outer = levels.get(outer)?.parent ?? null) if (dragged.has(outer)) return true;
    return false;
  };
  const callers = useRef(new Map<string, { fn: Fn; call: Fn }>());
  const steady = <D extends object>(id: string, data: D): D =>
    Object.fromEntries(
      Object.entries(data).map(([k, v]) => {
        if (typeof v !== 'function') return [k, v];
        const key = `${id} ${k}`;
        const caller: { fn: Fn; call: Fn } = callers.current.get(key) ?? { fn: v as Fn, call: (...args: never[]) => caller.fn(...args) };
        caller.fn = v as Fn;
        callers.current.set(key, caller);
        return [k, caller.call];
      }),
    ) as D;
  const keep = <N extends FlowNode>(node: N): N => {
    const live = flowState.get(node.id);
    const moved = carried.has(node.id) || insideDragged(node.id);
    return {
      ...node,
      data: steady(node.id, node.data),
      measured: live?.measured,
      ...((moved || node.id === chosenBox) && {
        className: [node.className, moved && 'carried', node.id === chosenBox && 'chosen'].filter(Boolean).join(' '),
      }),
      ...(live?.dragging && { position: node.parentId ? node.position : live.position, dragging: true, zIndex: DRAGGING_Z }),
    };
  };

  const repoNodes: RepoNode[] = framed
    ? repos.map((r) =>
        keep<RepoNode>({
          id: r.key,
          type: 'repo',
          position: repoAt.get(r.key)!,
          style: { width: r.size.width, height: r.size.height, pointerEvents: 'none' },
          zIndex: REPO_Z,
          dragHandle: '.repo-head',
          data: {
            name: r.repo.name,
            path: r.repo.path,
            wsl: r.repo.wsl,
            hasCommits: r.repo.hasCommits,
            creating: branching?.key === r.key,
            onArrange: () => arrangeRepo(r),
            onNewWorktree: (button) => {
              const rect = button.getBoundingClientRect();
              const at = flow.screenToFlowPosition({ x: rect.right, y: rect.bottom + POPOVER_GAP });
              const box = repoAt.get(r.key)!;
              setBranching({ key: r.key, anchor: { x: at.x - box.x, y: at.y - box.y } });
            },
          },
        }),
      )
    : [];
  const anchorAt = (origin: XYPosition, point: XYPosition, align: Align) => {
    const at = flow.screenToFlowPosition(point);
    setAnchor({ at: { x: at.x - origin.x, y: at.y - origin.y }, align });
  };
  const belowButton = (button: HTMLElement) => {
    const rect = button.getBoundingClientRect();
    return { x: rect.right, y: rect.bottom + POPOVER_GAP };
  };
  const isOpenFrom = (open: string | null, path: string, button: HTMLElement) => open === path && opener.current === button;
  const openNew = (path: string, origin: XYPosition, button: HTMLElement) => {
    setDeleting(null);
    if (isOpenFrom(creating, path, button)) return setCreating(null);
    opener.current = button;
    setCreating(path);
    anchorAt(origin, belowButton(button), 'end');
  };
  const openNewAt = (path: string, origin: XYPosition, point: XYPosition) => {
    setDeleting(null);
    opener.current = null;
    setCreating(path);
    anchorAt(origin, point, 'start');
  };
  const openDelete = (path: string, origin: XYPosition, button: HTMLElement) => {
    setCreating(null);
    if (isOpenFrom(deleting, path, button)) return setDeleting(null);
    opener.current = button;
    setDeleting(path);
    anchorAt(origin, belowButton(button), 'end');
  };
  const nodes: FlowNode[] = [...repoNodes, ...groups.flatMap(({ group, members, local, lead, origin, outer, size: box }) => [
    keep<GroupNode>({
      id: group.path,
      type: 'worktree',
      className: classes(group.path, litGroup(group.path, [...members.map((v) => String(v.id)), ...chipsOf(group.path).map((c) => c.path)])),
      ...(outer ? { parentId: outer.path, position: outer.at } : { position: origin }),
      style: members.length > 0 ? { width: box.width, height: box.height } : undefined,
      dragHandle: '.group-head',
      data: {
        branch: group.label,
        kind: group.kind,
        empty: members.length === 0,
        creating: creating === group.path,
        deleting: deleting === group.path,
        onArrange: () => arrangeGroup(group.path),
        onOpen: (button) => openNew(group.path, origin, button),
        onOpenAt: (point) => openNewAt(group.path, origin, point),
        onDelete: group.kind === 'linked' ? (button) => openDelete(group.path, origin, button) : undefined,
        chips: chipsOf(group.path).map((c) => ({
          path: c.path,
          label: c.label,
          className: [classes(c.path, lit(c.path)), chosenBox === c.path && 'chosen'].filter(Boolean).join(' ') || undefined,
          creating: creating === c.path,
          deleting: deleting === c.path,
        })),
        onOpenChip: (path, button) => openNew(path, origin, button),
        onDeleteChip: (path, button) => openDelete(path, origin, button),
        onChipRow: (height) => setChipRows((prev) => (prev.get(group.path) === height ? prev : new Map([...prev, [group.path, height]]))),
      },
    }),
    ...members.map((view): FlowNode => {
      const base = {
        id: String(view.id),
        parentId: group.path,
        className: classes(String(view.id), lit(String(view.id))),
        position: { x: local.get(String(view.id))!.x - lead.x, y: local.get(String(view.id))!.y - lead.y },
      };
      const data: CardBase = {
        view,
        defaultView: props.settings.cardView,
        agentLabel: agentLabel(props.agents, view.agent),
        onRefusal: setRefusal,
        selected: selected === view.id,
        focusAsk: selected === view.id ? props.focusAsk : 0,
        port: TREE_SIDES[direction].parent,
        onSelect: () => setSelected(view.id),
        onNavigate: navigate(view.id),
        onRemove: () => removeNode(view.id).catch((err: Error) => setRefusal(err.message)),
      };
      if (!standard(view.id)) return keep<SimpleNode>({ ...base, type: 'simple', data });
      const chosen = chosenSize(view.id);
      return keep<CardNode>({
        ...base,
        type: 'card',
        dragHandle: '.card-head',
        ...(chosen && { width: chosen.width, height: chosen.height, className: [base.className, 'sized'].filter(Boolean).join(' ') }),
        data: {
          ...data,
          sized: savedSize(view.id) !== undefined,
          onResizeStart: () => {
            const key = String(view.id);
            resizeFrom.current = { key, at: levels.get(key)!.boxes.find((b) => b.key === key)!.at };
          },
          onResize: (step) => {
            setResizing({ id: view.id, size: step.size });
            setMoving(resizeTo(String(view.id), step, moving));
          },
          onResized: (step) => {
            const rounded = { shift: { x: Math.round(step.shift.x), y: Math.round(step.shift.y) }, size: { width: Math.round(step.size.width), height: Math.round(step.size.height) } };
            setResizing(null);
            setMoving(new Map());
            commit(putSizes([{ node: view.id, ...rounded.size }]), { size: { id: view.id, size: rounded.size } });
            save(resizeTo(String(view.id), rounded, moving));
            resizeFrom.current = null;
          },
          onResetSize: () => resetSizes([view.id]),
          parentName: parentName(props.views, view),
          ownsPty: zoomed !== view.id,
          onExpand: () => {
            setSelected(view.id);
            setExpanded(view.id);
          },
        },
      });
    }),
  ])];

  const stepOf = (parent: Box, child: Box) => (props.settings.edgeStyle === 'step' ? stepAt(parent, child, direction, STEP_BEND) : null);
  const edges: Edge[] = views.flatMap((view) => {
    if (view.parent === null) return [];
    const pulse = props.pulses.get(view.id);
    const receivers = pulse ? ([['parent', view.parent], ['child', view.id]] as const).filter(([toward]) => pulse.counts[toward] > 0) : [];
    const color = pulse ? hopColor(pulse.hue) : undefined;
    const paint = color ? ({ '--hop': color } as CSSProperties) : undefined;
    const arrow = (toward: Toward) => (color && pulse?.counts[toward] ? arrowHead(color) : undefined);
    const from = positions.get(view.parent);
    const to = positions.get(view.id);
    const boxes = from && to ? ([{ at: from, size: size(view.parent) }, { at: to, size: size(view.id) }] as const) : null;
    const sides = boxes ? edgeSides(...boxes, direction) : TREE_SIDES[direction];
    const step = boxes && stepOf(...boxes);
    const chosen = selected === view.id || selected === view.parent;
    const ends = boxes && { parent: { box: boxes[0], side: sides.parent }, child: { box: boxes[1], side: sides.child } };
    const heading = (toward: Toward) => (ends ? labelHeading(toward === 'child' ? ends.parent : ends.child, ends[toward], step) : 'right');
    return [
      {
        id: `edge-${view.id}`,
        source: String(view.parent),
        target: String(view.id),
        sourceHandle: sides.parent,
        targetHandle: sides.child,
        ...(step === null || !boxes ? {} : { type: 'bend', data: { center: bendCenter(boxes[0], step, direction) } }),
        markerStart: arrow('parent'),
        markerEnd: arrow('child'),
        style: paint,
        labelBgStyle: paint,
        className: [chosen ? 'sel' : undefined, pulse ? 'live' : undefined, pulse && `to-${pulse.last}`, lit(String(view.id), String(view.parent))].filter(Boolean).join(' ') || undefined,
        zIndex: chosen || pulse ? RAISED_EDGE_Z : undefined,
        label: pulse ? receivers.map(([toward, id]) => t(`graph.messageTo.${heading(toward)}`, { name: props.views.get(id)?.name ?? `#${id}`, n: pulse.counts[toward] })).join(' · ') : undefined,
        labelBgPadding: [8, 2],
        labelBgBorderRadius: 10,
        focusable: false,
      },
    ];
  });

  const boxOf = (id: NodeId): Box | null => {
    const at = positions.get(id);
    return at ? { at, size: size(id) } : null;
  };
  const hopName = (id: NodeId) => props.views.get(id)?.name ?? `#${id}`;
  const centerOf = (id: NodeId): XYPosition | null => {
    const box = boxOf(id);
    return box && { x: box.at.x + box.size.width / 2, y: box.at.y + box.size.height / 2 };
  };
  const partsOf = (hop: Hop, turned: boolean) => hopParts(hop.counts, centerOf(hop.source), centerOf(hop.target), turned);
  const hopLabel = (hop: Hop, turned: boolean) =>
    partsOf(hop, turned)
      .map((part) => t(part.back ? 'graph.hopBack' : 'graph.hop', { from: hopName(hop[part.from]), to: hopName(hop[part.to]), n: hop.counts[part.to] }))
      .join(' / ');
  const hopShort = (hop: Hop, turned: boolean) =>
    partsOf(hop, turned)
      .map((part) => String(hop.counts[part.to]))
      .join(' / ');
  const roomOf = (id: NodeId, parent: NodeId, box: Box): Box => {
    const worktree = props.views.get(id)?.place.worktree;
    if (!worktree || worktree === props.views.get(parent)?.place.worktree) return box;
    const inset = insetOf(worktree);
    return { at: { x: box.at.x - inset.side, y: box.at.y - inset.top }, size: { width: box.size.width + 2 * inset.side, height: box.size.height + inset.top + inset.side } };
  };
  const turnedLabels = direction === 'right' || direction === 'left';
  const siblingPairs = new Map<NodeId, HopPair[]>();
  for (const [key, hop] of props.hops) {
    const parent = props.views.get(hop.source)?.parent ?? null;
    const source = boxOf(hop.source);
    const target = boxOf(hop.target);
    const up = parent === null ? null : boxOf(parent);
    if (parent === null || parent !== props.views.get(hop.target)?.parent || !source || !target || !up) continue;
    const end = (id: NodeId, box: Box) => ({ id, box, room: roomOf(id, parent, box), bend: stepOf(up, box) });
    siblingPairs.set(parent, [...(siblingPairs.get(parent) ?? []), { key, source: end(hop.source, source), target: end(hop.target, target), label: { full: hopLabelSize(hopLabel(hop, turnedLabels)), short: hopLabelSize(hopShort(hop, turnedLabels)) } }]);
  }
  const routes = new Map([...siblingPairs].flatMap(([parent, pairs]) => {
    const box = boxOf(parent);
    return box ? [...siblingLanes(box, pairs, direction)] : [];
  }));
  const hopEdges: HopEdgeType[] = [...props.hops].flatMap(([key, hop]) => {
    if (!props.views.has(hop.source) || !props.views.has(hop.target)) return [];
    const route = routes.get(key) ?? null;
    const text = hopLabel(hop, route?.turned ?? false);
    const color = hopColor(hop.hue);
    const arrow = (end: HopEndName) => (hop.counts[end] > 0 ? arrowHead(color) : undefined);
    return [
      {
        id: `hop-${key}`,
        type: 'hop',
        source: String(hop.source),
        target: String(hop.target),
        sourceHandle: 'top',
        targetHandle: 'top',
        data: { route, title: text, color },
        markerStart: arrow('source'),
        markerEnd: arrow('target'),
        className: ['live', 'hop', hop.last === 'target' ? 'to-child' : 'to-parent', lit(String(hop.source), String(hop.target))].filter(Boolean).join(' '),
        zIndex: RAISED_EDGE_Z,
        label: !route ? text : route.label === null ? undefined : route.label.short ? hopShort(hop, route.turned) : text,
        labelBgBorderRadius: 10,
        focusable: false,
      },
    ];
  });

  const boxAt = (node: FlowNode): XYPosition => {
    if (node.parentId) {
      const g = groups.find((g) => g.group.path === node.parentId)!;
      return { x: node.position.x + g.lead.x, y: node.position.y + g.lead.y };
    }
    if (node.type === 'worktree' && framed) {
      const r = repos.find((r) => r.laid.some((g) => g.group.path === node.id))!;
      const o = repoOrigin(r);
      return { x: node.position.x - o.x, y: node.position.y - o.y };
    }
    if (node.type === 'worktree') return { x: node.position.x - GROUP_POSITION.x, y: node.position.y - GROUP_POSITION.y };
    return node.position;
  };
  const originOf = (level: Level, key: string, at: XYPosition): XYPosition => {
    const lead = level.lead.get(key) ?? { x: 0, y: 0 };
    return { x: at.x - lead.x, y: at.y - lead.y };
  };
  const onDrag = (key: string, level: Level, dragged: Box) => {
    if (swaps.current?.key !== key) swaps.current = { key, swap: { slot: level.boxes.find((b) => b.key === key)!, sides: new Map(), at: new Map() } };
    const next = swap(swaps.current.swap, level.boxes.filter((b) => b.key !== key && (!alwaysArrange || branchOf.get(b.key) === branchOf.get(key))), dragged);
    swaps.current.swap = next;
    setMoving((prev) => new Map([...pinned(level, prev), [key, originOf(level, key, dragged.at)], ...[...next.at].map(([k, at]): [string, XYPosition] => [k, originOf(level, k, at)])]));
  };
  const pinned = (level: Level, prev: ReadonlyMap<string, XYPosition>): Map<string, XYPosition> => {
    const out = new Map(prev);
    for (const b of level.boxes) if (!out.has(b.key)) out.set(b.key, originOf(level, b.key, b.at));
    for (let parent = level.parent; parent !== null; ) {
      const outer = levels.get(parent);
      if (!outer) break;
      const self = outer.boxes.find((b) => b.key === parent)!;
      if (!out.has(parent)) out.set(parent, originOf(outer, parent, self.at));
      parent = outer.parent;
    }
    return out;
  };
  const onDrop = (key: string, level: Level, dragged: Box) => {
    swaps.current = null;
    setMoving(new Map());
    save(spread(key, level, dragged, new Map([...moving, [key, originOf(level, key, dragged.at)]])));
  };
  const placed = (level: Level, done: ReadonlyMap<string, XYPosition>) =>
    level.boxes.map((b) => {
      const o = done.get(b.key);
      const lead = level.lead.get(b.key) ?? { x: 0, y: 0 };
      return o ? { ...b, at: { x: o.x + lead.x, y: o.y + lead.y } } : b;
    });
  const pushOverlaps = ({ key, level, mover }: Step, done: Map<string, XYPosition>) => {
    const others = placed(level, done).filter((b) => b.key !== key);
    for (const [k, p] of push(others, mover, others.filter((o) => overlaps(o, mover)).map((o) => o.key), FREE_GAP)) done.set(k, originOf(level, k, p));
  };
  const shellOf = ({ key, level, mover }: Step, done: ReadonlyMap<string, XYPosition>): Step | null => {
    if (level.parent === null) return null;
    const outer = levels.get(level.parent);
    if (!outer) return null;
    const contents = placed(level, done).map((b) => (b.key === key ? { ...b, ...mover } : b));
    const shell = cover(contents, spotOf.get(level.parent)?.kind === 'worktree' ? insetOf(level.parent) : REPO_INSET);
    const self = outer.boxes.find((b) => b.key === level.parent)!;
    const origin = done.get(self.key) ?? originOf(outer, self.key, self.at);
    return { key: self.key, level: outer, mover: { at: { x: origin.x + shell.at.x, y: origin.y + shell.at.y }, size: shell.size } };
  };
  const spread = (key: string, level: Level, mover: Box, done: Map<string, XYPosition>): Map<string, XYPosition> => {
    for (let s: Step | null = { key, level, mover }; s !== null; s = shellOf(s, done)) pushOverlaps(s, done);
    return done;
  };
  const save = (moved: ReadonlyMap<string, XYPosition>) => {
    const done = new Map(
      [...moved].filter(([k, p]) => {
        const draft = draftAt.get(k);
        return draft === undefined || !samePoint(draft, p);
      }),
    );
    if (done.size > 0) saveAfter(done, Promise.resolve());
  };
  const saveAfter = (done: ReadonlyMap<string, XYPosition>, after: Promise<unknown>) => {
    const placed = place(done);
    const inside = new Set(placed.flatMap((p) => (p.spot.kind === 'repo' ? [] : (repoOf(p.spot) ?? []))));
    if (alwaysArrange) return keepArranged([...inside], { spots: done }, after);
    commit(after.then(() => putSpots(placed)), { spots: done });
  };
  const keepArranged = (ids: readonly RepoId[], draft: Draft & { spots: ReadonlyMap<string, XYPosition> }, after: Promise<unknown> = Promise.resolve()) => {
    const placed = place(draft.spots);
    commit(after.then(() => putArranged({ repos: [...ids], placed })), draft);
  };
  const place = (done: ReadonlyMap<string, XYPosition>): Placed[] => [...done].map(([k, p]): Placed => ({ spot: spotOf.get(k)!, at: p }));
  const keepAround = (key: string, newLead: XYPosition): Map<string, XYPosition> => {
    const level = levels.get(key)!;
    const self = level.boxes.find((b) => b.key === key)!;
    return new Map([...pinned(level, new Map()), [key, { x: self.at.x - newLead.x, y: self.at.y - newLead.y }]]);
  };
  const [ordering, setOrdering] = useState<string | null>(null);
  const arrangeGroup = (path: string) => {
    holdOrder();
    setOrdering(path);
  };
  useEffect(() => {
    if (ordering === null) return;
    setOrdering(null);
    const g = groups.find((g) => g.group.path === ordering);
    if (!g) return;
    const done = new Map([...keepAround(ordering, { x: 0, y: 0 }), ...g.auto]);
    if (arrangedNow(g.repo)) keepArranged([g.repo], { spots: done });
    else save(done);
  });
  const arrangeRepo = (r: (typeof repos)[number]) => {
    const kept = keepAround(r.key, { x: -REPO_INSET.side, y: -REPO_INSET.top });
    holdOrder();
    const after = clearSpots(r.repo.id);
    saveAfter(kept, after);
    setArranging({ scope: { kind: 'repo', id: r.repo.id }, after });
  };
  useEffect(() => {
    if (arranging === null) return;
    setArranging(null);
    const { scope } = arranging;
    const done = scope.kind === 'shown' ? repos : repos.filter((r) => r.repo.id === scope.id);
    const boxes = scope.kind === 'shown' && framed ? repos.map((r): [string, XYPosition] => [r.key, { x: repoAuto.get(r.key)!.x - r.lead.x, y: repoAuto.get(r.key)!.y - r.lead.y }]) : [];
    const ids = done.map((r) => r.repo.id);
    keepArranged(ids, { spots: new Map([...done.flatMap((r) => [...r.arranged]), ...boxes]), clearing: ids }, arranging.after);
  });
  useEffect(() => {
    outers.current = outerNow;
    if (stale.size > 0) setSpots((prev) => new Map([...prev].filter(([key]) => !stale.has(key))));
  });

  const lastFlow = useRef(flowState);
  const onNodesChange = (changes: NodeChange<FlowNode>[]) => {
    const next = new Map(applyNodeChanges(changes, nodes.map((n) => lastFlow.current.get(n.id) ?? n)).map((n) => [n.id, n]));
    lastFlow.current = next;
    setFlowState(next);
    for (const c of changes) {
      if (c.type !== 'position' || c.id === resizeFrom.current?.key) continue;
      const node = next.get(c.id);
      const level = levels.get(c.id);
      if (!node || !level) continue;
      const dragged = { at: boxAt(node), size: level.boxes.find((b) => b.key === c.id)!.size };
      if (c.dragging) onDrag(c.id, level, dragged);
      else if (moving.has(c.id)) onDrop(c.id, level, dragged);
    }
  };
  const latestChange = useRef(onNodesChange);
  latestChange.current = onNodesChange;
  const onChange = useCallback((changes: NodeChange<FlowNode>[]) => latestChange.current(changes), []);

  const store = useStoreApi();
  const remeasure = useUpdateNodeInternals();
  useEffect(() => {
    const lost = [...store.getState().nodeLookup.values()].filter((n) => n.measured.width && !n.internals.handleBounds).map((n) => n.id);
    if (lost.length > 0) remeasure(lost);
  });

  const onConnectEnd = (event: MouseEvent | TouchEvent, connection: FinalConnectionState) => {
    if (!connection.fromNode) return;
    const point = 'changedTouches' in event ? event.changedTouches[0] : event;
    const target = document.elementFromPoint(point.clientX, point.clientY)?.closest('.react-flow__node-card, .react-flow__node-simple')?.getAttribute('data-id');
    if (!target || target === connection.fromNode.id) return;
    adoptNode(Number(target), Number(connection.fromNode.id)).catch((err: Error) => setRefusal(err.message));
  };

  const zoomedView = zoomed === null || !standard(zoomed) ? undefined : props.views.get(zoomed);

  return (
    <>
      <ReactFlow
        ref={pane}
        nodes={nodes}
        edges={[...edges, ...hopEdges]}
        edgeTypes={EDGE_TYPES}
        nodeTypes={nodeTypes}
        onNodesChange={onChange}
        onPaneClick={() => {
          props.onClear();
          props.onClearHighlight();
          setCreating(null);
          setDeleting(null);
          setRefusal('');
        }}
        onNodeClick={(event, node) => {
          if (node.type === 'card' || node.type === 'simple') return isControl(event.target) ? undefined : setSelected(Number(node.id));
          setChosenBox(node.id);
        }}
        onMoveStart={(event) => {
          cameraMoving.current = true;
          document.documentElement.classList.toggle('panning', event?.type === 'mousedown');
        }}
        onMoveEnd={() => {
          cameraMoving.current = false;
          document.documentElement.classList.remove('panning');
          realignTerminals();
        }}
        onNodeDragStop={() => realignTerminals()}
        onConnectStart={() => setRefusal('')}
        onConnectEnd={onConnectEnd}
        zIndexMode="manual"
        elementsSelectable={false}
        nodesFocusable={false}
        deleteKeyCode={null}
        zoomOnDoubleClick={false}
        minZoom={MIN_ZOOM}
        maxZoom={MAX_ZOOM}
      >
        {!minimapFolded && (
          <MiniMap
            pannable
            ariaLabel={t('graph.minimap')}
            nodeClassName={(node) => (node.type === 'repo' || node.type === 'worktree' ? node.type : minimapClass((node as CardNode | SimpleNode).data.view))}
            onClick={(_, position) => flow.setCenter(position.x, position.y, { zoom: flow.getZoom(), duration: MOVE_MS })}
          />
        )}
        <UnzoomVar />
        <ZoomLevel
          folded={minimapFolded}
          onFold={props.onMinimapFold}
        />
        {groups.flatMap(({ group, repo, members, origin }) => [{ group, repo, members, origin }, ...chipsOf(group.path).map((chip) => ({ group: chip, repo, members: [], origin }))]).map(({ group, repo, members, origin }) => {
          return (
            (creating === group.path || deleting === group.path) &&
            anchor && (
              <ViewportPortal key={group.path}>
                <PopAt
                  at={{ x: origin.x + anchor.at.x, y: origin.y + anchor.at.y }}
                  align={anchor.align}
                  opener={opener}
                  onClose={() => {
                    setCreating(null);
                    setDeleting(null);
                  }}
                >
                  {creating === group.path ? (
                    <NewNodePopover
                      branch={group.label}
                      place={{ repo, worktree: group.path }}
                      distro={props.repos.find((r) => r.id === repo)?.wsl?.distro ?? null}
                      agents={props.agents}
                      onOpen={props.onRescanAgents}
                      recent={recentAgent(views)}
                      settings={props.settings}
                      accounts={props.accounts}
                      onCreated={(id) => {
                        setCreating(null);
                        setSelected(id);
                      }}
                      onCancel={() => setCreating(null)}
                    />
                  ) : (
                    <DeleteWorktreePopover
                      repo={repo}
                      group={group}
                      views={members}
                      onDeleted={(repo) => {
                        setDeleting(null);
                        props.onRepo(repo);
                      }}
                      onClose={() => setDeleting(null)}
                    />
                  )}
                </PopAt>
              </ViewportPortal>
            )
          );
        })}
        {framed &&
          repos.map(
            (r) =>
              branching?.key === r.key && (
                <ViewportPortal key={r.key}>
                  <PopAt at={{ x: repoAt.get(r.key)!.x + branching.anchor.x, y: repoAt.get(r.key)!.y + branching.anchor.y }} align="end" onClose={() => setBranching(null)}>
                    <NewWorktreePopover
                      repo={r.repo}
                      workspaceDir={props.settings.workspaceDir ?? ''}
                      onClose={() => setBranching(null)}
                      onCreated={(repo) => {
                        setBranching(null);
                        props.onRepo(repo);
                      }}
                    />
                  </PopAt>
                </ViewportPortal>
              ),
          )}
      </ReactFlow>
      {refusal && (
        <p className="refusal" role="alert">
          {refusal}
        </p>
      )}
      {zoomedView && (
        <div className="zoomed">
          <NodeCard
            view={zoomedView}
            agentLabel={agentLabel(props.agents, zoomedView.agent)}
            parentName={parentName(props.views, zoomedView)}
            selected={selected === zoomedView.id}
            onSelect={() => setSelected(zoomedView.id)}
            onNavigate={() => {}}
            onDoubleClickHead={() => setExpanded(null)}
            headAction={
              <>
                <BookmarkButton view={zoomedView} />
                <IconButton label={t('node.collapse')} title={t('node.collapse')} onClick={() => setExpanded(null)}>
                  <Minimize2 />
                </IconButton>
                <NodeMenu
                  view={zoomedView}
                  defaultView={props.settings.cardView}
                  onRemove={() => {
                    setExpanded(null);
                    removeNode(zoomedView.id).catch((err: Error) => setRefusal(err.message));
                  }}
                  onRefusal={setRefusal}
                />
              </>
            }
          >
            <SizingTerminal src={nodePty(zoomedView.id)} node={zoomedView.id} focused={selected === zoomedView.id} focusAsk={props.focusAsk} onFocus={() => setSelected(zoomedView.id)} onNavigate={() => {}} />
          </NodeCard>
        </div>
      )}
    </>
  );
}
