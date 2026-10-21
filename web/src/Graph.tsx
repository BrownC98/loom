import { GitBranchPlus, Maximize2, Minimize2, Plus, RotateCcw, Trash2, Workflow } from 'lucide-react';
import { Fragment, memo, useCallback, useEffect, useImperativeHandle, useLayoutEffect, useRef, useState, type ReactNode, type Ref } from 'react';
import { useTranslation } from 'react-i18next';
import { applyNodeChanges, Handle, MiniMap, NodeResizeControl, Panel, Position, ResizeControlVariant, ReactFlow, useReactFlow, useStore, useStoreApi, useUpdateNodeInternals, ViewportPortal, type Edge, type FinalConnectionState, type Node, type NodeChange, type NodeProps, type XYPosition } from '@xyflow/react';
import { adoptNode, clearSizes, clearSpots, getSizes, getSpots, nodePty, putSizes, putSpots, removeNode } from './api';
import { agentLabel, recentAgent } from './catalog';
import type { AccountView } from './gen/AccountView';
import type { AgentView } from './gen/AgentView';
import type { CardView } from './gen/CardView';
import type { Toward } from './gen/Toward';
import type { NodeId } from './gen/NodeId';
import type { RepoId } from './gen/RepoId';
import type { NodeView } from './gen/NodeView';
import type { Placed } from './gen/Placed';
import type { RepoView } from './gen/RepoView';
import type { SettingsView } from './gen/SettingsView';
import type { Spot } from './gen/Spot';
import { DeleteWorktreePopover } from './DeleteWorktreePopover';
import { arrangeTree, centerSide, crossed, edgeSides, FREE_GAP, freeSpot, GROUP_GAP, grew, keepsArranged, overlaps, placeGroups, push, SIDES, stepAt, TREE_SIDES, type ArrangeDirection, type Box, type CenterSide, type Side, type Sibling, type Size } from './layout';
import { NewNodePopover } from './NewNodePopover';
import { NewWorktreePopover } from './NewWorktreePopover';
import { NodeCard, SimpleCard } from './NodeCard';
import { attachCards, groupParents, groupsOf, inGroup, type Group, type GroupKind } from './repos';
import { Terminal, type Direction } from './Terminal';

// 배치 값은 승인한 목업(docs/mockups/main.html)의 좌표에서 가져왔다.
const GROUP_POSITION: XYPosition = { x: 16, y: 64 };
/**
 * 층(z). React Flow는 `zIndexMode="manual"`이라 이 값을 그대로 쓴다. 카드는 묶음 z + 1이다(React Flow 규칙).
 * 저장소 상자 < 간선 = 묶음 < 고른 카드의 간선 = 카드 < 끄는 상자. 같은 z면 간선이 노드 아래다(DOM 순서).
 * 간선은 묶음 머리를 덮지 않는다. 고른 카드의 간선과 메시지가 지나는 간선만 묶음 위로 올린다
 * (2026-10-21 사람 승인, 같은 날 "메시지 이동시에는 해당 관계선을 묶음 위로 보이게 해줘").
 */
const REPO_Z = -1;
const RAISED_EDGE_Z = 1;
/** 끄는 상자의 z. 그 묶음의 카드보다 높아야 한다. */
const DRAGGING_Z = 2000;
/** 빈 묶음을 재기 전에 쓰는 크기(머리와 안내 글). 재면 잰 크기다. 빈 묶음은 카드 폭을 차지하지 않는다(목업 docs/mockups/repo-drag.html 안 A). */
const EMPTY_GROUP: Size = { width: 320, height: 100 };
/** 카드와 묶음 테두리 사이 틈. 왼쪽·오른쪽·아래가 같다. */
const CARD_X = 20;
const CARD_TOP = 60;
/** 나무 간선이 부모 카드에서 꺾이는 거리. 묶음 아래 틈(카드 틈 `CARD_X` + 묶음 틈 절반)의 가운데라 자식 묶음 머리 앞에서 꺾는다(목업 docs/mockups/arrange-mixed.html v5). */
const STEP_BEND = CARD_X + GROUP_GAP / 2;
/** 묶음 안 카드 줄 사이 틈. 예전에 줄마다 두던 자식 묶음 머리·테두리와 묶음 틈의 합과 같다. */
const CARD_ROW_GAP: Record<'vertical' | 'horizontal', number> = { vertical: CARD_TOP + CARD_X + GROUP_GAP, horizontal: 2 * CARD_X + GROUP_GAP };
/**
 * 카드를 재기 전에 쓰는 크기. 재면 잰 크기로 배치한다. `width`는 styles.css `.card`·`.card.simple`의 폭이다.
 * 표준 높이는 머리, 80×24 터미널(styles.css `.term`), 발 줄의 합이다. 심플 높이는 목업 docs/mockups/card-view.html에서 쟀다.
 */
const CARD_SIZE: Record<CardView, Size> = {
  standard: { width: 600, height: 466 },
  simple: { width: 260, height: 134 },
};
/** 사람이 줄일 수 있는 가장 작은 표준 카드. 머리 줄과 터미널 8줄이다. 더 좁으면 머리의 이름과 버튼이 겹친다(목업 docs/mockups/card-resize.html). 가장 큰 크기는 없다. */
const MIN_CARD: Size = { width: 360, height: 200 };
const POSITION: Record<Side, Position> = { top: Position.Top, right: Position.Right, bottom: Position.Bottom, left: Position.Left };
/** [새 노드]·[새 워크트리] 버튼 아래로 띄우는 팝오버의 틈. 팝오버 오른쪽 끝을 버튼 오른쪽 끝에 맞춘다. */
const POPOVER_GAP = 6;
/** 팝오버와 그래프 칸 왼쪽 끝 사이에 남기는 틈(화면 px). */
const POPOVER_EDGE = 8;
/** 묶음 오른쪽 위에서 잰 삭제 경고 위치. 경고는 [워크트리 삭제] 아래에 오른쪽을 맞춰 연다. */
const DELETE_OFFSET: XYPosition = { x: -8, y: 40 };
/** 저장소 상자 머리 높이(테두리 줄 포함). styles.css `.repo-head` 높이와 같다. */
const REPO_HEAD = 46;
/** 저장소 상자 테두리와 묶음 사이 틈. `top`은 머리를 포함한다. 머리 아래 틈은 옆 틈과 같다(2026-10-21 사람 지시 "상단 간격 좀 띄워, 좌우 패딩값과 일치하게"). */
const REPO_SIDE = 16;
const REPO_INSET = { top: REPO_HEAD + REPO_SIDE, side: REPO_SIDE };
/** 노드로 옮겨 가는 화면 이동 시간. */
const MOVE_MS = 300;
/**
 * 손으로 바꾸는 확대·축소 한계. 축소: 2026-10-21 사람 지시 "터미널 노드가 대략 뒤의 좌표점 2*3 크기로 보일만큼".
 * 바탕 점 간격은 화면 20px(styles.css `.canvas .react-flow`)이다. 0.1배면 기본 카드(600×466)가 60×47px, 점 3×2칸이다.
 * 확대: 같은 날 "줌인은 350% 까지로 변경"(전: 4배). 3.5배면 기본 카드(600px)가 2100px 칸을 채운다. 맞춰 보기(fitView)는 1을 넘기지 않는다.
 */
const MIN_ZOOM = 0.1;
const MAX_ZOOM = 3.5;

/**
 * 그래프 위 팝오버. `at`(흐름 좌표)에 팝오버 오른쪽 끝을 맞춘다.
 * 그러면 팝오버가 그래프 칸 왼쪽 끝을 넘을 때 넘는 만큼 오른쪽으로 민다. 확대·이동할 때마다 다시 잰다.
 */
function PopAt(props: { at: XYPosition; children: ReactNode }) {
  const box = useRef<HTMLDivElement>(null);
  const [shift, setShift] = useState(0);
  const transform = useStore((s) => s.transform);
  useLayoutEffect(() => {
    const pop = box.current?.firstElementChild;
    const pane = box.current?.closest('.react-flow');
    if (!pop || !pane) return;
    const zoom = transform[2];
    // 민 만큼을 빼서 밀기 전 왼쪽 끝을 잰다.
    const left = pop.getBoundingClientRect().left - shift * zoom;
    const next = Math.max(0, (pane.getBoundingClientRect().left + POPOVER_EDGE - left) / zoom);
    // 0.5px 아래 차이는 버린다. 소수점 오차로 재기와 다시 그리기가 되풀이되지 않게 한다.
    if (Math.abs(next - shift) > 0.5) setShift(next);
  }, [props.at.x, props.at.y, transform, shift]);
  return (
    <div ref={box} className="pop-end" style={{ position: 'absolute', transform: `translate(${props.at.x + shift}px, ${props.at.y}px)` }}>
      {props.children}
    </div>
  );
}

/** 카드 머리 아이콘 단추(목업 docs/mockups/main.html). 아이콘은 자식으로 받는다. */
/**
 * 지금 배율(2026-10-21 사람 지시 "현재 배율을 미니맵에 표시해줘"). React Flow 미니맵은 자식을 받지 않는다.
 * 그래서 같은 오른쪽 아래 패널로 미니맵 위 오른쪽 아래 모서리에 겹쳐 그린다. 숫자 밖 클릭은 미니맵이 받는다.
 */
function ZoomLevel() {
  const { t } = useTranslation();
  const flow = useReactFlow();
  const zoom = useStore((s) => s.transform[2]);
  // 더블클릭하면 숫자를 바로 고친다(같은 날 "배율 더블클릭시 직접 수정 가능하게"). Enter로 바꾸고 Esc·바깥 클릭으로 그만둔다.
  // 한계 밖 값은 React Flow가 MIN_ZOOM~MAX_ZOOM으로 자른다.
  const [draft, setDraft] = useState<string | null>(null);
  const apply = () => {
    const percent = Number(draft);
    if (draft !== null && draft.trim() !== '' && Number.isFinite(percent) && percent > 0) flow.zoomTo(percent / 100, { duration: MOVE_MS });
    setDraft(null);
  };
  return (
    <Panel position="bottom-right" className="zoom-level">
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

/** 그래프 화면을 `ids` 노드가 모두 보이게 옮긴다. 그래프 밖(트리)에서 노드로 옮겨 갈 때 쓴다. `ReactFlowProvider` 안에서만 부른다. */
export function useShowNodes(): (ids: NodeId[]) => void {
  const flow = useReactFlow();
  return (ids) => flow.fitView({ nodes: ids.map((id) => ({ id: String(id) })), duration: MOVE_MS, maxZoom: 1 });
}

/** 그래프 화면을 `paths` 워크트리 묶음이 모두 보이게 옮긴다. 묶음 노드의 id는 워크트리 경로다. 아직 그리지 않은 묶음이면(저장소를 막 바꿈) React Flow가 노드를 잰 뒤 옮긴다. */
export function useShowGroups(): (paths: string[]) => void {
  const flow = useReactFlow();
  return (paths) => flow.fitView({ nodes: paths.map((id) => ({ id })), duration: MOVE_MS, maxZoom: 1 });
}

/** `onDelete`가 없으면 지울 수 없다(원래 체크아웃). */
type GroupData = {
  branch: string;
  kind: GroupKind;
  empty: boolean;
  creating: boolean;
  deleting: boolean;
  /** 누른 [새 노드] 버튼. 창을 그 버튼 옆에 띄우려고 자리를 잰다. */
  onOpen: (button: HTMLElement) => void;
  onDelete?: () => void;
  /** 이 묶음 안 카드만 자동 배치로 놓는다(목업 docs/mockups/repo-drag.html v5 ⑥). */
  onArrange: () => void;
};
type CardBase = { view: NodeView; agentLabel: string; selected: boolean; port: Side; onSelect: () => void; onNavigate: (dir: Direction) => void; onRemove: () => void };
/**
 * 표준 카드. `ownsPty`: 이 카드가 터미널을 연다. 확대 창이 열면 카드는 터미널을 그리지 않는다. PTY 크기를 한쪽만 정하게 한다.
 * `sized`: 사람이 크기를 바꿨다. 그 카드만 [기본 크기로 되돌리기]가 보인다(목업 docs/mockups/card-resize.html ③).
 * `onResize`: 테두리를 끄는 동안 크기. `onResized`: 놓은 크기. 저장한다.
 */
type CardData = CardBase & { parentName: string | null; ownsPty: boolean; onExpand: () => void; sized: boolean; onResize: (size: Size) => void; onResized: (size: Size) => void; onResetSize: () => void };
/**
 * `onArrange`: 이 저장소의 묶음과 카드만 자동 배치로 놓는다(목업 docs/mockups/repo-drag.html v5 ⑦).
 * `onNewWorktree`: 누른 [새 워크트리] 버튼 아래에 새 워크트리 창을 연다(목업 docs/mockups/repo-new-worktree.html). 첫 커밋 전에는 버튼을 끈다.
 */
type RepoData = { name: string; path: string; hasCommits: boolean; creating: boolean; onArrange: () => void; onNewWorktree: (button: HTMLElement) => void };
type GroupNode = Node<GroupData, 'worktree'>;
/** 모든 저장소 범위에서 저장소마다 묶음을 두르는 상자. 머리를 끌어 옮긴다. */
type RepoNode = Node<RepoData, 'repo'>;
type CardNode = Node<CardData, 'card'>;
type SimpleNode = Node<CardBase, 'simple'>;
type FlowNode = RepoNode | GroupNode | CardNode | SimpleNode;
/** 노드 data에 담는 처리기. */
type Fn = (...args: never[]) => unknown;

function Group({ data }: NodeProps<GroupNode>) {
  const { t } = useTranslation();
  return (
    <section className="group" aria-label={t('graph.group', { branch: data.branch })}>
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
          <button className="btn ghost sm nodrag" aria-expanded={data.deleting} onClick={data.onDelete}>
            {t('deleteWorktree.action')}
          </button>
        )}
      </div>
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
        <span className="sub" translate="no" title={data.path}>
          {data.path}
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

/** 크기를 바꾸는 테두리: 오른쪽 변, 아래 변, 오른쪽 아래 모서리. 보이지 않고 커서만 바뀐다(styles.css `.react-flow__resize-control`). */
const RESIZE_EDGES = [
  { position: 'right', variant: ResizeControlVariant.Line },
  { position: 'bottom', variant: ResizeControlVariant.Line },
  { position: 'bottom-right', variant: ResizeControlVariant.Handle },
] as const;

function Card({ data }: NodeProps<CardNode>) {
  const { t } = useTranslation();
  // 끄는 동안 터미널 칸을 시작 크기에 묶는다. 그러면 터미널 열·줄(PTY 크기)은 놓을 때 한 번 바뀐다.
  const termBox = useRef<HTMLDivElement>(null);
  const [hold, setHold] = useState<Size | null>(null);
  return (
    <>
      {RESIZE_EDGES.map((edge) => (
        <NodeResizeControl
          key={edge.position}
          position={edge.position}
          variant={edge.variant}
          minWidth={MIN_CARD.width}
          minHeight={MIN_CARD.height}
          onResizeStart={() => {
            const el = termBox.current;
            if (el) setHold({ width: el.offsetWidth, height: el.offsetHeight });
          }}
          onResize={(_, p) => data.onResize({ width: p.width, height: p.height })}
          onResizeEnd={(_, p) => {
            setHold(null);
            data.onResized({ width: p.width, height: p.height });
          }}
        />
      ))}
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
            {data.sized && (
              <IconButton label={t('node.resetSize')} title={t('node.resetSize')} onClick={data.onResetSize}>
                <RotateCcw />
              </IconButton>
            )}
            <IconButton label={t('node.expand')} title={t('node.expandTitle')} onClick={data.onExpand}>
              <Maximize2 />
            </IconButton>
            <RemoveButton onRemove={data.onRemove} />
          </>
        }
      >
        <div ref={termBox} className="term-hold" style={hold ?? undefined}>
          {data.ownsPty && <Terminal src={nodePty(data.view.id)} focused={data.selected} onFocus={data.onSelect} onNavigate={data.onNavigate} />}
        </div>
      </NodeCard>
      <Ports port={data.port} />
    </>
  );
}

function Simple({ data }: NodeProps<SimpleNode>) {
  return (
    <>
      <SimpleCard view={data.view} agentLabel={data.agentLabel} selected={data.selected} onSelect={data.onSelect} onNavigate={data.onNavigate} actions={<RemoveButton onRemove={data.onRemove} />} />
      <Ports port={data.port} />
    </>
  );
}

function RemoveButton(props: { onRemove: () => void }) {
  const { t } = useTranslation();
  return (
    <IconButton label={t('node.remove')} title={t('node.remove')} onClick={props.onRemove}>
      <Trash2 />
    </IconButton>
  );
}

/**
 * 간선이 붙는 자리. 변마다 들어오는 핸들과 나가는 핸들을 두고, 간선이 `edgeSides`로 고른다(id는 변 이름).
 * 잇기 점은 `port` 변 하나다. 그 점을 끌어 다른 카드에 놓으면 그 카드의 부모가 된다(onConnectEnd).
 */
function Ports(props: { port: Side }) {
  return SIDES.map((side) => (
    <Fragment key={side}>
      <Handle type="target" id={side} position={POSITION[side]} isConnectable={false} />
      <Handle type="source" id={side} position={POSITION[side]} isConnectable={side === props.port} className={side === props.port ? 'port' : undefined} />
    </Fragment>
  ));
}

/** 노드는 data 값이 바뀔 때만 다시 그린다. 끄는 동안 자리만 바뀐 노드는 그대로 둔다. data의 함수는 `steady`가 늘 같은 함수로 준다. */
function sameData(a: { data: object }, b: { data: object }): boolean {
  const [x, y] = [a.data as Record<string, unknown>, b.data as Record<string, unknown>];
  const keys = Object.keys(x);
  return keys.length === Object.keys(y).length && keys.every((k) => x[k] === y[k]);
}

const nodeTypes = { repo: memo(Repo, sameData), worktree: memo(Group, sameData), card: memo(Card, sameData), simple: memo(Simple, sameData) };

/** `from`에서 `dir` 쪽으로 가장 가까운 카드. 가로·세로 어긋남은 두 배로 친다. */
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

/** 자리의 React Flow 노드 id. 묶음 노드의 id는 워크트리 경로다. */
export function flowId(spot: Spot): string {
  return spot.kind === 'repo' ? `repo:${spot.repo}` : spot.kind === 'worktree' ? spot.path : String(spot.node);
}

/** `contents`(그 상자 안 좌표)를 모두 덮는 상자. `inset`은 내용과 테두리 사이 틈이다(`top`은 머리를 포함한다). */
function cover(contents: readonly Box[], inset: { top: number; side: number }): Box {
  const x = Math.min(...contents.map((b) => b.at.x)) - inset.side;
  const y = Math.min(...contents.map((b) => b.at.y)) - inset.top;
  return {
    at: { x, y },
    size: { width: Math.max(...contents.map((b) => b.at.x + b.size.width)) + inset.side - x, height: Math.max(...contents.map((b) => b.at.y + b.size.height)) + inset.side - y },
  };
}

/**
 * 한 좌표계의 형제 상자들(같은 묶음의 카드, 같은 저장소의 묶음, 저장소 상자). 끄는 상자는 이 상자들과 겹치지 않게 한다.
 * `lead`는 상자 원점에서 상자 왼쪽 위까지다(카드는 0). 자리는 원점으로 저장한다. `parent`는 이 상자들을 덮는 바깥 상자의 id다.
 */
type Level = { boxes: Sibling[]; lead: ReadonlyMap<string, XYPosition>; parent: string | null };

/** 강조하는 상자(React Flow 노드 id, `flowId`). `current`는 찾기의 현재 일치이고 테두리를 하나 더 그린다. */
export type Lit = { keys: ReadonlySet<string>; current: string | null };

/** 강조하는 동안 한 간선으로 오간 메시지 수. 가는 쪽마다 센다. */
export type Pulse = Readonly<Record<Toward, number>>;

/**
 * `neighbor`: 그래프 밖(오른쪽 칸의 터미널)에서 옆 카드를 고를 때 쓴다. `arrange`: 끌어 놓은 자리를 지우고 자동 배치로 되돌린다(오른쪽 사이드바의 [정렬]).
 * `hasSizes`·`resetSizes`: 보이는 카드 가운데 크기를 바꾼 카드가 있는지, 그 카드들을 기본 크기로 되돌린다([정렬] 메뉴, 목업 docs/mockups/card-resize.html ④).
 */
export type GraphHandle = { neighbor: (from: NodeId, dir: Direction) => NodeId | null; arrange: () => void; hasSizes: () => boolean; resetSizes: () => void };

/**
 * `cardView`: 그래프 전체의 카드 보기. 심플이면 카드에 터미널이 없고, 터미널은 부르는 쪽(오른쪽 칸)이 연다.
 * `pulses`: 자식 id → 강조하는 동안 그 간선으로 오간 메시지 수(가는 쪽마다). `agents`·`settings`·`accounts`는 카드 이름과 새 노드 창에 쓴다.
 * `selected`: 입력을 받는 카드. 트리도 고르므로 부르는 쪽이 가진다.
 * `highlight`: 이벤트 콘솔에서 고른 이벤트에 관여한 노드, 또는 찾기의 일치(노드와 묶음). 그 상자와 그 사이 간선을 강조하고 나머지는 흐리게 한다.
 * 바탕을 누르면 `onClearHighlight`를 부른다.
 */
export function Graph(props: {
  /** 그래프가 그리는 저장소. 둘 이상이면 저장소마다 상자를 두른다. `views`는 이 저장소들의 노드만이다. */
  repos: readonly RepoView[];
  onRepo: (repo: RepoView) => void;
  views: ReadonlyMap<NodeId, NodeView>;
  pulses: ReadonlyMap<NodeId, Pulse>;
  agents: AgentView[];
  settings: SettingsView;
  accounts: AccountView[] | null;
  selected: NodeId | null;
  onSelect: (id: NodeId | null) => void;
  highlight: Lit | null;
  onClearHighlight: () => void;
  cardView: CardView;
  /** 나무가 뻗는 쪽. 오른쪽 사이드바의 [정렬] 메뉴가 고른다. */
  direction: ArrangeDirection;
  ref?: Ref<GraphHandle>;
}) {
  const { t } = useTranslation();
  const { highlight } = props;
  /** 강조 중이면 `keys`가 모두 강조한 상자인 쪽은 `hl`, 아니면 `dim`. */
  const lit = (...keys: string[]) => (highlight === null ? undefined : keys.every((key) => highlight.keys.has(key)) ? 'hl' : 'dim');
  /** 묶음은 강조한 카드를 담으면 흐리게 하지 않는다. */
  const litGroup = (path: string, members: NodeView[]) => (highlight !== null && !highlight.keys.has(path) && members.some((v) => highlight.keys.has(String(v.id))) ? undefined : lit(path));
  const classes = (key: string, base: string | undefined) => [base, highlight?.current === key ? 'cur' : undefined].filter(Boolean).join(' ') || undefined;
  const { selected, onSelect: setSelected } = props;
  const flow = useReactFlow();
  const [expanded, setExpanded] = useState<NodeId | null>(null);
  // 새 노드 창이나 삭제 경고를 연 묶음(워크트리 경로).
  const [creating, setCreating] = useState<string | null>(null);
  // [새 노드] 버튼의 오른쪽 아래 모서리(묶음 왼쪽 위에서 잰 자리). 창을 그 자리에 오른쪽을 맞춰 띄운다. 묶음을 끌면 함께 움직인다.
  const [anchor, setAnchor] = useState<XYPosition | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  // 새 워크트리 창을 연 저장소 상자와 [새 워크트리] 버튼의 오른쪽 아래 모서리(상자 왼쪽 위에서 잰 자리). 상자를 끌면 창도 함께 움직인다.
  const [branching, setBranching] = useState<{ key: string; anchor: XYPosition } | null>(null);
  // 입양을 데몬이 거절한 문장. 다음 끌기나 바탕 클릭까지 보인다.
  const [refusal, setRefusal] = useState('');
  // React Flow가 잰 크기. 노드를 새 객체로 넘겨도 이 값을 이어받아야 React Flow가 노드를 다시 숨기고 재지 않는다.
  const [flowState, setFlowState] = useState<ReadonlyMap<string, FlowNode>>(new Map());
  // 사람이 끌어 놓은 자리(React Flow 노드 id → 저장한 자리). 원점의 기준은 `Spot`이 적는다.
  const [spots, setSpots] = useState<ReadonlyMap<string, Placed>>(new Map());
  // 끄는 동안 움직인 상자(끄는 상자와 비킨 상자)의 원점. 놓으면 저장하고 비운다.
  const [moving, setMoving] = useState<ReadonlyMap<string, XYPosition>>(new Map());
  // 끄는 상자 중심이 형제 중심선의 어느 쪽에 있었는지. 겹친 채 쪽이 바뀌면 그 형제가 비킨다(사람 결정 2026-10-21, 목업 v4 ②).
  const sides = useRef<{ key: string; of: Map<string, CenterSide> } | null>(null);
  // 사람이 바꾼 표준 카드 크기와, 테두리를 끄는 동안의 크기. 놓으면 `sizes`에 넣고 저장한다.
  const [sizes, setSizes] = useState<ReadonlyMap<NodeId, Size>>(new Map());
  const [resizing, setResizing] = useState<{ id: NodeId; size: Size } | null>(null);
  useEffect(() => {
    getSpots()
      .then((all) => setSpots(new Map(all.map((p) => [flowId(p.spot), p]))))
      .catch((err: Error) => setRefusal(err.message));
    getSizes()
      .then((all) => setSizes(new Map(all.map((s) => [s.node, { width: s.width, height: s.height }]))))
      .catch((err: Error) => setRefusal(err.message));
  }, []);
  const { direction } = props;
  const placedAt = (key: string): XYPosition | undefined => moving.get(key) ?? spots.get(key)?.at;

  // [정렬]을 누를 때 화면에 보이던 카드·묶음의 가로지르는 축 자리(카드는 노드 번호, 묶음은 경로가 키). 자동 배치는 형제를 이 순서로 놓는다(목업 docs/mockups/arrange-wide.html 규칙 4).
  // [정렬]은 이 순서로 구한 자리를 저장하므로(`arranging`) 다시 열어도 순서가 남는다.
  const [order, setOrder] = useState<ReadonlyMap<string, number>>(new Map());
  const rank = (key: string) => order.get(key) ?? Infinity;
  const byRank = <T,>(items: readonly T[], key: (item: T) => string, tie: (a: T, b: T) => number) => [...items].sort((a, b) => rank(key(a)) - rank(key(b)) || tie(a, b));
  const views = byRank([...props.views.values()], (v) => String(v.id), (a, b) => a.id - b.id);
  const estimate = CARD_SIZE[props.cardView];
  const standard = props.cardView === 'standard';
  /** 사람이 정한 표준 카드 크기. 없으면 CSS가 정한 기본 크기다. */
  const chosenSize = (id: NodeId): Size | undefined => (!standard ? undefined : resizing?.id === id ? resizing.size : sizes.get(id));
  /** 잰 카드 크기. 재기 전에는 사람이 정한 크기, 그것도 없으면 보기마다 정한 크기다. */
  const size = (id: NodeId): Size => {
    const measured = flowState.get(String(id))?.measured;
    return measured?.width && measured.height ? { width: measured.width, height: measured.height } : (chosenSize(id) ?? estimate);
  };
  // 상자 id → 그 상자의 형제들. 끄는 동안과 놓을 때 쓴다. `spotOf`는 상자 id → 저장할 자리 이름이다.
  const levels = new Map<string, Level>();
  const spotOf = new Map<string, Spot>();
  const share = (level: Level) => {
    for (const b of level.boxes) levels.set(b.key, level);
  };
  /**
   * 형제 상자의 자리. 저장한 자리가 있으면 그 자리, 없으면 자동 배치 자리다. 저장한 형제가 있으면 새 상자는 겹치지 않는 오른쪽으로 간다.
   * `auto`와 돌려주는 값은 상자 왼쪽 위다.
   */
  const settle = (items: readonly { key: string; size: Size; lead: XYPosition; auto: XYPosition }[]): Map<string, XYPosition> => {
    const at = new Map<string, XYPosition>();
    const taken: Box[] = [];
    for (const item of items) {
      const saved = placedAt(item.key);
      if (saved) at.set(item.key, { x: saved.x + item.lead.x, y: saved.y + item.lead.y });
    }
    for (const [key, p] of at) taken.push({ at: p, size: items.find((i) => i.key === key)!.size });
    for (const item of items) {
      if (at.has(item.key)) continue;
      const p = freeSpot({ at: item.auto, size: item.size }, taken, FREE_GAP);
      at.set(item.key, p);
      taken.push({ at: p, size: item.size });
    }
    return at;
  };
  /** 묶음 하나의 카드 자리(묶음 원점 기준)와 묶음 상자(원점 기준 `lead`와 크기). */
  const layGroup = (group: Group, members: NodeView[], bands: ReadonlyMap<NodeId, number>) => {
    const vertical = direction === 'down' || direction === 'up';
    const { cards: tree, bands: slots } = arrangeTree(members, size, direction, CARD_ROW_GAP[vertical ? 'vertical' : 'horizontal'], bands);
    // 나무 배치는 카드 자리 기준이라 `CARD_X`·`CARD_TOP`만큼 옮긴다. 가장 왼쪽·위 카드를 그 틈에 맞춘다.
    const min = { x: Math.min(...[...tree.values()].map((p) => p.x)), y: Math.min(...[...tree.values()].map((p) => p.y)) };
    const auto = new Map(members.map((v) => [String(v.id), { x: CARD_X + tree.get(v.id)!.x - min.x, y: CARD_TOP + tree.get(v.id)!.y - min.y }]));
    const local = settle(members.map((v) => ({ key: String(v.id), size: size(v.id), lead: { x: 0, y: 0 }, auto: auto.get(String(v.id))! })));
    for (const v of members) spotOf.set(String(v.id), { kind: 'node', repo: v.place.repo, node: v.id });
    share({ boxes: members.map((v) => ({ key: String(v.id), at: local.get(String(v.id))!, size: size(v.id) })), lead: new Map(), parent: group.path });
    // 묶음은 카드를 모두 덮는다(네 쪽). 빈 묶음은 머리와 안내 글 크기다(높이·폭을 비워 내용에 맞춘다).
    const measured = flowState.get(group.path)?.measured;
    const box =
      members.length > 0
        ? cover(members.map((v) => ({ at: local.get(String(v.id))!, size: size(v.id) })), { top: CARD_TOP, side: CARD_X })
        : { at: { x: 0, y: 0 }, size: { width: measured?.width ?? EMPTY_GROUP.width, height: measured?.height ?? EMPTY_GROUP.height } };
    // 자식 묶음 줄 자리의 시작(묶음 상자 기준). 사람이 카드를 옮겼으면 줄 자리도 카드를 따라간다.
    const cross = (p: XYPosition) => (vertical ? p.x : p.y);
    const band = (card: NodeId) => cross(local.get(String(card))!) - cross(box.at) + slots.get(card)! - cross(tree.get(card)!);
    return { group, members, local, auto, lead: box.at, size: box.size, band };
  };
  // 저장소가 둘 이상이면(모든 저장소 범위) 저장소마다 상자를 두르고 그 안에 묶음을 놓는다(목업 docs/mockups/repo-scope.html).
  const framed = props.repos.length > 1;
  const repos = props.repos.map((repo) => {
    const repoViews = views.filter((v) => v.place.repo === repo.id);
    const repoGroups = byRank(groupsOf(repo, repoViews), (g) => g.path, () => 0);
    // 자동 배치는 부모 카드 아래 줄에 자식 묶음을 놓는 나무다. 자리는 저장소 원점 기준 묶음 상자 왼쪽 위다.
    const parents = groupParents(repoGroups, repoViews);
    const cards = attachCards(repoGroups, repoViews, parents);
    const laid: ReturnType<typeof layGroup>[] = [];
    const auto = placeGroups(
      repoGroups.map((_, i) => ({ parent: parents[i], card: cards[i] })),
      (i, bands) => {
        const g = (laid[i] = layGroup(repoGroups[i], inGroup(repoViews, repoGroups[i]), bands));
        return { size: g.size, band: g.band };
      },
      direction,
    );
    const at = settle(laid.map((g, i) => ({ key: g.group.path, size: g.size, lead: g.lead, auto: auto[i] })));
    for (const g of laid) spotOf.set(g.group.path, { kind: 'worktree', repo: repo.id, path: g.group.path });
    const key = `repo:${repo.id}`;
    share({ boxes: laid.map((g) => ({ key: g.group.path, at: at.get(g.group.path)!, size: g.size })), lead: new Map(laid.map((g) => [g.group.path, g.lead])), parent: framed ? key : null });
    // 저장소 상자는 묶음을 모두 덮는다(네 쪽). 끄는 카드로 묶음이 넓어지면 함께 넓어진다.
    const box = cover(laid.map((g) => ({ at: at.get(g.group.path)!, size: g.size })), REPO_INSET);
    // 자동 배치 자리를 저장할 원점(카드는 묶음 기준, 묶음은 저장소 기준). [정렬]이 저장한다.
    const arranged = new Map([...laid.flatMap((g) => [...g.auto]), ...laid.map((g, i): [string, XYPosition] => [g.group.path, { x: auto[i].x - g.lead.x, y: auto[i].y - g.lead.y }])]);
    return { repo, key, laid: laid.map((g) => ({ ...g, at: at.get(g.group.path)! })), lead: box.at, size: box.size, arranged };
  });
  // 저장소 상자도 같은 규칙이다. 자동 배치는 모두 뿌리라 사이드바 순서로 옆에 늘어선다.
  const repoAuto = placeGroups(
    repos.map(() => ({ parent: null, card: null })),
    (i) => ({ size: repos[i].size, band: () => 0 }),
    direction,
  ).map((p) => ({ x: GROUP_POSITION.x + p.x, y: GROUP_POSITION.y + p.y }));
  const repoAt = settle(repos.map((r, i) => ({ key: r.key, size: r.size, lead: r.lead, auto: repoAuto[i] })));
  for (const r of repos) spotOf.set(r.key, { kind: 'repo', repo: r.repo.id });
  if (framed) share({ boxes: repos.map((r) => ({ key: r.key, at: repoAt.get(r.key)!, size: r.size })), lead: new Map(repos.map((r) => [r.key, r.lead])), parent: null });
  // 저장소 원점(그래프 좌표). 저장소 하나면 상자가 없고 원점은 `GROUP_POSITION`이다.
  const repoOrigin = (r: (typeof repos)[number]) => (framed ? { x: repoAt.get(r.key)!.x - r.lead.x, y: repoAt.get(r.key)!.y - r.lead.y } : GROUP_POSITION);
  // 묶음 상자 왼쪽 위(그래프 좌표, `origin`)와 원점(`base`, 카드 자리의 기준). 카드 위치는 묶음 상자 기준이다. `positions`는 키보드 이동에 쓰는 화면 기준이다.
  const positions = new Map<NodeId, XYPosition>();
  const groups = repos.flatMap((r) => {
    const o = repoOrigin(r);
    return r.laid.map((g) => {
      const origin = { x: o.x + g.at.x, y: o.y + g.at.y };
      for (const [id, p] of g.local) positions.set(Number(id), { x: origin.x + p.x - g.lead.x, y: origin.y + p.y - g.lead.y });
      return { ...g, repo: r.repo.id, origin, right: g.size.width };
    });
  });
  /** 보이는 카드·묶음의 가로지르는 축 자리. [정렬]이 형제 순서로 쓴다(`order`). */
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
    // [정렬]: 보이는 저장소의 자리를 지운다. 지우지 못한 저장소는 다음에 다시 읽으면 저장한 자리로 돌아간다.
    arrange: () => {
      const shown = props.repos.map((r) => r.id);
      setMoving(new Map());
      clearRepos(shown);
      setArranging({ scope: { kind: 'shown' }, after: Promise.all(shown.map(clearSpots)) });
    },
    hasSizes: () => views.some((v) => sizes.has(v.id)),
    resetSizes: () => resetSizes(views.filter((v) => sizes.has(v.id)).map((v) => v.id)),
  }));
  // 크기만 되돌린다. 카드 자리와 밀린 이웃은 그대로다.
  const resetSizes = (ids: NodeId[]) => {
    if (ids.length === 0) return;
    setSizes((prev) => new Map([...prev].filter(([id]) => !ids.includes(id))));
    clearSizes(ids).catch((err: Error) => setRefusal(err.message));
  };
  /** 크기를 바꾸는 카드가 `next` 크기일 때 겹친 이웃과 커진 바깥 상자의 옆 상자를 민다. 줄여도 민 상자는 돌아오지 않는다. */
  const resizeTo = (id: NodeId, next: Size, done: ReadonlyMap<string, XYPosition>) => {
    const key = String(id);
    const level = levels.get(key)!;
    const self = level.boxes.find((b) => b.key === key)!;
    return spread(key, level, { at: self.at, size: next }, pinned(level, done));
  };
  // 끄는 상자는 React Flow가 옮긴 자리에 그린다. React Flow가 그 노드에 `dragging` 클래스를 붙여 반투명하게 보인다.
  // 끄는 카드는 계산한 자리다. React Flow 자리는 넓어지기 전 묶음 기준이라 묶음이 넓어지면 어긋난다.
  // 늘 맨 위다(2026-10-20 사람 지시 "드래그로 잡은건 겹칠때 항상 최상위 z 인덱스 적용"). 묶음을 끌면 그 카드도 함께 올라온다.
  // 끄는 상자와 함께 움직이는 상자(끄는 묶음의 카드, 끄는 상자를 덮는 묶음·저장소 상자)는 옮김 애니메이션을 끈다. 켜 두면 커서보다 늦게 따라온다.
  const carried = new Set<string>();
  for (const [id, n] of flowState) {
    if (!n.dragging) continue;
    for (let outer = levels.get(id)?.parent ?? null; outer !== null; outer = levels.get(outer)?.parent ?? null) carried.add(outer);
  }
  // data의 함수를 노드·이름마다 같은 함수로 바꾼다. 부르면 마지막 렌더의 함수를 부른다. 그래서 끄는 동안 다른 노드는 다시 그리지 않는다(`sameData`).
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
    const moved = carried.has(node.id) || (node.parentId !== undefined && flowState.get(node.parentId)?.dragging);
    return {
      ...node,
      data: steady(node.id, node.data),
      measured: live?.measured,
      ...(moved && { className: [node.className, 'carried'].filter(Boolean).join(' ') }),
      ...(live?.dragging && { position: node.parentId ? node.position : live.position, dragging: true, zIndex: DRAGGING_Z }),
    };
  };

  // 저장소 상자를 먼저 넣어 묶음과 카드 아래에 그린다.
  const repoNodes: RepoNode[] = framed
    ? repos.map((r) =>
        keep<RepoNode>({
          id: r.key,
          type: 'repo',
          position: repoAt.get(r.key)!,
          // 상자는 머리만 클릭을 받는다(머리를 끌어 옮긴다). 상자 안 빈 곳 클릭은 바탕 클릭이다(고르기를 푼다).
          style: { width: r.size.width, height: r.size.height, pointerEvents: 'none' },
          zIndex: REPO_Z,
          dragHandle: '.repo-head',
          data: {
            name: r.repo.name,
            path: r.repo.path,
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
  const nodes: FlowNode[] = [...repoNodes, ...groups.flatMap(({ group, members, local, lead, origin, size: box }) => [
    keep<GroupNode>({
      id: group.path,
      type: 'worktree',
      className: classes(group.path, litGroup(group.path, members)),
      position: origin,
      style: members.length > 0 ? { width: box.width, height: box.height } : undefined,
      // 묶음 머리를 끌어 묶음을 옮긴다. 카드는 묶음 기준 위치라 함께 옮겨진다.
      dragHandle: '.group-head',
      data: {
        branch: group.label,
        kind: group.kind,
        empty: members.length === 0,
        creating: creating === group.path,
        deleting: deleting === group.path,
        onArrange: () => arrangeGroup(group.path),
        onOpen: (button) => {
          setDeleting(null);
          setCreating(group.path);
          const rect = button.getBoundingClientRect();
          const at = flow.screenToFlowPosition({ x: rect.right, y: rect.bottom + POPOVER_GAP });
          setAnchor({ x: at.x - origin.x, y: at.y - origin.y });
        },
        onDelete:
          group.kind === 'linked'
            ? () => {
                setCreating(null);
                setDeleting(group.path);
              }
            : undefined,
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
        agentLabel: agentLabel(props.agents, view.agent),
        selected: selected === view.id,
        port: TREE_SIDES[direction].parent,
        onSelect: () => setSelected(view.id),
        onNavigate: navigate(view.id),
        // 확인 없이 바로 지운다(docs/design.md 「노드 제거와 입양」). 실패하면 입양 거절과 같은 자리에 보인다.
        onRemove: () => removeNode(view.id).catch((err: Error) => setRefusal(err.message)),
      };
      // 심플 카드는 카드 전체가 끌기 손잡이다. 표준 카드는 터미널을 고를 수 있게 머리만 손잡이다.
      if (!standard) return keep<SimpleNode>({ ...base, type: 'simple', data });
      const chosen = chosenSize(view.id);
      return keep<CardNode>({
        ...base,
        type: 'card',
        dragHandle: '.card-head',
        ...(chosen && { width: chosen.width, height: chosen.height, className: [base.className, 'sized'].filter(Boolean).join(' ') }),
        data: {
          ...data,
          sized: sizes.has(view.id),
          onResize: (next) => {
            setResizing({ id: view.id, size: next });
            setMoving(resizeTo(view.id, next, moving));
          },
          onResized: (next) => {
            const rounded = { width: Math.round(next.width), height: Math.round(next.height) };
            setResizing(null);
            setSizes((prev) => new Map([...prev, [view.id, rounded]]));
            putSizes([{ node: view.id, ...rounded }]).catch((err: Error) => setRefusal(err.message));
            setMoving(new Map());
            save(resizeTo(view.id, rounded, moving));
          },
          onResetSize: () => resetSizes([view.id]),
          parentName: parentName(props.views, view),
          ownsPty: expanded !== view.id,
          onExpand: () => {
            setSelected(view.id);
            setExpanded(view.id);
          },
        },
      });
    }),
  ])];

  const edges: Edge[] = views.flatMap((view) => {
    if (view.parent === null) return [];
    const pulse = props.pulses.get(view.id);
    const receivers = pulse ? ([['parent', view.parent], ['child', view.id]] as const).filter(([toward]) => pulse[toward] > 0) : [];
    // 한쪽으로만 갔으면 점선이 받는 쪽으로 흐른다. 경로는 늘 부모 → 자식으로 그린다.
    const flow = receivers.length === 1 ? `to-${receivers[0][0]}` : undefined;
    const from = positions.get(view.parent);
    const to = positions.get(view.id);
    const boxes = from && to ? ([{ at: from, size: size(view.parent) }, { at: to, size: size(view.id) }] as const) : null;
    const sides = boxes ? edgeSides(...boxes, direction) : TREE_SIDES[direction];
    const step = boxes && props.settings.edgeStyle === 'step' ? stepAt(...boxes, direction, STEP_BEND) : null;
    const chosen = selected === view.id || selected === view.parent;
    return [
      {
        id: `edge-${view.id}`,
        source: String(view.parent),
        target: String(view.id),
        sourceHandle: sides.parent,
        targetHandle: sides.child,
        ...(step === null ? {} : { type: 'smoothstep', pathOptions: { offset: 0, stepPosition: step } }),
        className: [chosen ? 'sel' : undefined, pulse ? 'live' : undefined, flow, lit(String(view.id), String(view.parent))].filter(Boolean).join(' ') || undefined,
        zIndex: chosen || pulse ? RAISED_EDGE_Z : undefined,
        label: pulse ? receivers.map(([toward, id]) => t('graph.messageTo', { name: props.views.get(id)?.name ?? `#${id}`, n: pulse[toward] })).join(' · ') : undefined,
        labelBgPadding: [8, 2],
        labelBgBorderRadius: 10,
        focusable: false,
      },
    ];
  });

  /** 상자의 지금 자리(형제와 같은 좌표의 상자 왼쪽 위). React Flow 자리는 카드면 묶음 상자 기준, 아니면 그래프 기준이다. */
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
  /** 끄는 동안: 끄는 상자 중심이 겹친 형제의 중심선을 넘으면 그 형제가 비킨다(목업 v4 ②). */
  const onDrag = (key: string, level: Level, dragged: Box) => {
    const others = level.boxes.filter((b) => b.key !== key);
    if (sides.current?.key !== key) sides.current = { key, of: new Map() };
    const of = sides.current.of;
    const targets: string[] = [];
    for (const o of others) {
      const now = centerSide(dragged, o);
      const before = of.get(o.key);
      if (!overlaps(dragged, o) || before === undefined) of.set(o.key, now);
      else if (crossed(before, now)) targets.push(o.key);
    }
    const pushed = push(others, dragged, targets, FREE_GAP);
    for (const [k, at] of pushed) of.set(k, centerSide(dragged, { at, size: others.find((o) => o.key === k)!.size }));
    setMoving((prev) => new Map([...pinned(level, prev), [key, originOf(level, key, dragged.at)], ...[...pushed].map(([k, at]): [string, XYPosition] => [k, originOf(level, k, at)])]));
  };
  /**
   * 형제와 바깥 상자(묶음, 저장소 상자)를 지금 자리에 묶는다. 자리를 저장하지 않은 상자는 자동 배치 자리라 끄는 상자를 피해 옮겨 가고,
   * 바깥 상자는 왼쪽 위가 고정이라 안의 상자가 왼쪽·위로 가면 원점이 따라 움직인다. 그래서 끄는 동안 지금 자리에 고정하고, 놓으면 함께 저장한다.
   */
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
  /**
   * 놓을 때: 끄는 상자는 그 자리에 선다. 아직 겹친 형제는 같은 규칙으로 비킨다. 커진 바깥 상자(묶음, 저장소 상자)가 옆 상자와 겹치면 그 상자도 비킨다.
   * 놓은 상자와 비킨 상자의 자리를 한 요청으로 저장한다.
   */
  const onDrop = (key: string, level: Level, dragged: Box) => {
    sides.current = null;
    setMoving(new Map());
    save(spread(key, level, dragged, new Map([...moving, [key, originOf(level, key, dragged.at)]])));
  };
  /** `key` 상자가 `mover` 자리·크기일 때 겹친 형제를 비키고, 커진 바깥 상자도 같은 규칙으로 따른다. `done`(상자 → 원점)에 더해 돌려준다. */
  const spread = (key: string, level: Level, mover: Box, done: Map<string, XYPosition>): Map<string, XYPosition> => {
    let [at, lv] = [key, level];
    for (;;) {
      const boxes = lv.boxes.map((b) => {
        const o = done.get(b.key);
        const lead = lv.lead.get(b.key) ?? { x: 0, y: 0 };
        return o ? { ...b, at: { x: o.x + lead.x, y: o.y + lead.y } } : b;
      });
      const others = boxes.filter((b) => b.key !== at);
      const pushed = push(others, mover, others.filter((o) => overlaps(o, mover)).map((o) => o.key), FREE_GAP);
      for (const [k, p] of pushed) done.set(k, originOf(lv, k, p));
      if (lv.parent === null) break;
      const outer = levels.get(lv.parent);
      if (!outer) break;
      // 바깥 상자 = 원점 + 안의 상자를 덮는 상자.
      const contents = boxes.map((b) => (pushed.has(b.key) ? { ...b, at: pushed.get(b.key)! } : b.key === at ? { ...b, ...mover } : b));
      const shell = cover(contents, spotOf.get(lv.parent)?.kind === 'worktree' ? { top: CARD_TOP, side: CARD_X } : REPO_INSET);
      const self = outer.boxes.find((b) => b.key === lv.parent)!;
      const origin = done.get(self.key) ?? originOf(outer, self.key, self.at);
      [at, mover, lv] = [self.key, { at: { x: origin.x + shell.at.x, y: origin.y + shell.at.y }, size: shell.size }, outer];
    }
    return done;
  };
  /** 상자 원점을 한 요청으로 저장한다. `after`가 있으면 그 요청보다 먼저 보낸다. */
  const save = (done: ReadonlyMap<string, XYPosition>, after: Promise<unknown> = Promise.resolve()) => {
    if (done.size === 0) return;
    const placed = [...done].map(([k, p]): Placed => ({ spot: spotOf.get(k)!, at: p }));
    setSpots((prev) => new Map([...prev, ...placed.map((p): [string, Placed] => [flowId(p.spot), p])]));
    after.then(() => putSpots(placed)).catch((err: Error) => setRefusal(err.message));
  };
  // 부분 정렬(목업 v5 ⑥⑦). 정렬한 상자의 왼쪽 위와 바깥·형제 상자는 지금 자리에 둔다. 다음 렌더에서 커진 상자가 옆 상자를 비킨다(`lastSizes`).
  const keepAround = (key: string, newLead: XYPosition): Map<string, XYPosition> => {
    const level = levels.get(key)!;
    const self = level.boxes.find((b) => b.key === key)!;
    return new Map([...pinned(level, new Map()), [key, { x: self.at.x - newLead.x, y: self.at.y - newLead.y }]]);
  };
  // 자동 배치 카드 자리는 카드 틈에서 시작하므로 묶음 상자 왼쪽 위가 원점이다.
  // 묶음 정렬은 저장한 카드 자리를 자동 자리로 바꾼다. 자동 자리를 지금 화면 순서로 다시 구한 다음 렌더에서 저장한다(`ordering`).
  const [ordering, setOrdering] = useState<string | null>(null);
  const arrangeGroup = (path: string) => {
    setOrder((prev) => new Map([...prev, ...onScreen()]));
    setOrdering(path);
  };
  useEffect(() => {
    if (ordering === null) return;
    setOrdering(null);
    const g = groups.find((g) => g.group.path === ordering);
    if (!g) return;
    save(new Map([...keepAround(ordering, { x: 0, y: 0 }), ...g.auto]));
  });
  // 자동 배치 묶음 자리는 저장소 원점에서 시작하므로 상자 왼쪽 위는 원점에서 `REPO_INSET`만큼 왼쪽 위다.
  const arrangeRepo = (r: (typeof repos)[number]) => {
    const kept = keepAround(r.key, { x: -REPO_INSET.side, y: -REPO_INSET.top });
    clearRepos([r.repo.id]);
    const after = clearSpots(r.repo.id);
    save(kept, after);
    setArranging({ scope: { kind: 'repo', id: r.repo.id }, after });
  };
  /**
   * [정렬]과 저장소 정렬. 지금 화면 순서를 잡고 저장한 자리를 지운다. 다음 렌더에서 구한 자동 배치 자리를 저장한다.
   * 그래서 순서와 자리는 데몬 DB(`spots`) 하나에 남고, 브라우저와 Windows 앱이 같은 그림을 본다.
   * 보이는 저장소 모두(`shown`)면 저장소 상자 자리도 저장한다. 저장소 하나면 그 상자는 지금 자리에 남는다(`keepAround`). `after`는 지우기 요청이다.
   */
  const [arranging, setArranging] = useState<{ scope: { kind: 'shown' } | { kind: 'repo'; id: RepoId }; after: Promise<unknown> } | null>(null);
  const clearRepos = (ids: readonly RepoId[]) => {
    setOrder((prev) => new Map([...prev, ...onScreen()]));
    setSpots((prev) => new Map([...prev].filter(([, p]) => !ids.includes(p.spot.repo))));
  };
  useEffect(() => {
    if (arranging === null) return;
    setArranging(null);
    const { scope } = arranging;
    const done = scope.kind === 'shown' ? repos : repos.filter((r) => r.repo.id === scope.id);
    const boxes = scope.kind === 'shown' && framed ? repos.map((r, i): [string, XYPosition] => [r.key, { x: repoAuto[i].x - r.lead.x, y: repoAuto[i].y - r.lead.y }]) : [];
    save(new Map([...done.flatMap((r) => [...r.arranged]), ...boxes]), arranging.after);
  });
  // 끌지 않고 커진 상자(새 카드·묶음이 든 바깥 상자, 카드·터미널 크기, 정렬)는 끌어 놓을 때와 같은 규칙으로 겹친 옆 상자를 민다.
  // 끄는 동안과 테두리를 끄는 동안(`moving`)은 놓을 때 민다. 크기만 기억한다. 안쪽 상자부터 민다(`levels`는 카드, 묶음, 저장소 순서다).
  // 정렬한 모양을 지키는 저장소(`keepsArranged`)는 밀지 않고 새 자동 배치를 저장한다. 그 저장소의 상자는 앞 크기를 기억해 두고,
  // 새 자동 배치로 그린 다음 렌더에서 그 크기와 견주어 커진 저장소 상자가 옆 저장소를 민다.
  const lastSizes = useRef<ReadonlyMap<string, Size>>(new Map());
  const lastArranged = useRef<ReadonlyMap<RepoId, ReadonlyMap<string, XYPosition>>>(new Map());
  useEffect(() => {
    const now = new Map([...levels].map(([key, level]): [string, Size] => [key, level.boxes.find((b) => b.key === key)!.size]));
    const before = lastSizes.current;
    lastSizes.current = now;
    const arranged = lastArranged.current;
    lastArranged.current = new Map(repos.map((r) => [r.repo.id, r.arranged]));
    if (moving.size > 0) return;
    const saved = new Map([...spots].map(([key, p]) => [key, p.at]));
    const kept = repos.filter((r) => keepsArranged(saved, arranged.get(r.repo.id) ?? new Map(), r.arranged));
    const done = new Map(kept.flatMap((r) => [...r.arranged]));
    const skip = new Set(kept.flatMap((r) => [r.key, ...r.arranged.keys()]));
    lastSizes.current = new Map([...now].map(([key, size]) => [key, (skip.has(key) && before.get(key)) || size]));
    for (const key of grew(before, now)) {
      if (skip.has(key)) continue;
      // 앞 상자가 이미 민 상자는 민 자리에서 민다.
      const level = levels.get(key)!;
      const self = level.boxes.find((b) => b.key === key)!;
      const o = done.get(key);
      const lead = level.lead.get(key) ?? { x: 0, y: 0 };
      spread(key, level, o ? { at: { x: o.x + lead.x, y: o.y + lead.y }, size: self.size } : self, done);
    }
    save(done);
  });

  // 바뀜은 마지막 결과에 더한다. 렌더 전에 바뀜이 여럿 오면(끄는 자리, 그래서 커진 바깥 상자의 크기) 렌더한 노드에 더할 때
  // 앞 바뀜을 잃는다. 그러면 끄는 상자가 앞 자리로 돌아가 커서를 따르지 않는다(바깥 상자 벽에 닿을 때).
  const lastFlow = useRef(flowState);
  const onNodesChange = (changes: NodeChange<FlowNode>[]) => {
    const next = new Map(applyNodeChanges(changes, nodes.map((n) => lastFlow.current.get(n.id) ?? n)).map((n) => [n.id, n]));
    lastFlow.current = next;
    setFlowState(next);
    // 카드·묶음·저장소 상자를 끌 때만 위치 변경이 온다(키보드 이동은 껐다).
    for (const c of changes) {
      if (c.type !== 'position') continue;
      const node = next.get(c.id);
      const level = levels.get(c.id);
      if (!node || !level) continue;
      const dragged = { at: boxAt(node), size: level.boxes.find((b) => b.key === c.id)!.size };
      if (c.dragging) onDrag(c.id, level, dragged);
      // 끌지 않고 머리를 누르기만 하면 자리를 저장하지 않는다.
      else if (moving.has(c.id)) onDrop(c.id, level, dragged);
    }
  };
  // React Flow는 처리기도 effect에서 받는다. 부하가 크면 effect가 늦어 지난 렌더의 처리기가 불린다. 그 처리기는 지난 `moving`으로
  // 놓은 자리를 저장한다. 그래서 늘 마지막 렌더의 처리기를 부른다.
  const latestChange = useRef(onNodesChange);
  latestChange.current = onNodesChange;
  const onChange = useCallback((changes: NodeChange<FlowNode>[]) => latestChange.current(changes), []);

  // React Flow는 `nodes`를 effect에서 받는다. 새 카드를 재기 전에 시작한 렌더의 effect가 잰 뒤에 돌면
  // `measured` 없는 노드가 잰 핸들 위치를 지운다. 그 뒤 크기가 돌아와도 크기가 그대로라 다시 재지 않아 간선이 빠진다.
  // 자식(React Flow)의 effect가 먼저 돈다. 그래서 여기서 그 상태를 찾아 다시 재게 한다.
  const store = useStoreApi();
  const remeasure = useUpdateNodeInternals();
  useEffect(() => {
    const lost = [...store.getState().nodeLookup.values()].filter((n) => n.measured.width && !n.internals.handleBounds).map((n) => n.id);
    if (lost.length > 0) remeasure(lost);
  });

  /** 잇기 점을 놓은 자리의 카드가 끌기 시작한 카드를 새 부모로 받는다. 대상 핸들 대신 카드 전체를 놓는 자리로 쓴다. */
  const onConnectEnd = (event: MouseEvent | TouchEvent, connection: FinalConnectionState) => {
    if (!connection.fromNode) return;
    const point = 'changedTouches' in event ? event.changedTouches[0] : event;
    const target = document.elementFromPoint(point.clientX, point.clientY)?.closest('.react-flow__node-card, .react-flow__node-simple')?.getAttribute('data-id');
    if (!target || target === connection.fromNode.id) return;
    adoptNode(Number(target), Number(connection.fromNode.id)).catch((err: Error) => setRefusal(err.message));
  };

  const zoomedView = expanded === null || !standard ? undefined : props.views.get(expanded);

  return (
    <>
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        onNodesChange={onChange}
        onPaneClick={() => {
          setSelected(null);
          props.onClearHighlight();
          setCreating(null);
          setDeleting(null);
          setRefusal('');
        }}
        // 카드 머리는 끌기 손잡이라 React Flow가 mousedown 전파를 막는다. 머리 클릭은 여기서 고른다.
        onNodeClick={(_, node) => (node.type === 'card' || node.type === 'simple') && setSelected(Number(node.id))}
        onConnectStart={() => setRefusal('')}
        onConnectEnd={onConnectEnd}
        onlyRenderVisibleElements
        zIndexMode="manual"
        elementsSelectable={false}
        nodesFocusable={false}
        deleteKeyCode={null}
        zoomOnDoubleClick={false}
        minZoom={MIN_ZOOM}
        maxZoom={MAX_ZOOM}
      >
        <MiniMap
          pannable
          ariaLabel={t('graph.minimap')}
          nodeClassName={(node) => (node.type === 'repo' || node.type === 'worktree' ? node.type : (node as CardNode | SimpleNode).data.view.state.kind)}
          onClick={(_, position) => flow.setCenter(position.x, position.y, { zoom: flow.getZoom(), duration: MOVE_MS })}
        />
        <ZoomLevel />
        {groups.map(({ group, repo, members, origin, right }) => {
          const at = creating === group.path && anchor ? { x: origin.x + anchor.x, y: origin.y + anchor.y } : null;
          return (
            // 묶음 노드 안에 두면 자식 카드가 팝오버를 덮는다. 뷰포트 층에 그려 카드 위에 둔다.
            (creating === group.path || deleting === group.path) && (
              <ViewportPortal key={group.path}>
                {/* 새 노드 창은 누른 [새 노드] 버튼의 오른쪽 아래 모서리에 오른쪽을 맞춰 붙인다. */}
                <PopAt at={at ?? { x: origin.x + right + DELETE_OFFSET.x, y: origin.y + DELETE_OFFSET.y }}>
                  {creating === group.path ? (
                    <NewNodePopover
                      branch={group.label}
                      place={{ repo, worktree: group.path }}
                      agents={props.agents}
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
                  <PopAt at={{ x: repoAt.get(r.key)!.x + branching.anchor.x, y: repoAt.get(r.key)!.y + branching.anchor.y }}>
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
              <IconButton label={t('node.collapse')} title={t('node.collapse')} onClick={() => setExpanded(null)}>
                <Minimize2 />
              </IconButton>
            }
          >
            <Terminal src={nodePty(zoomedView.id)} focused={selected === zoomedView.id} onFocus={() => setSelected(zoomedView.id)} onNavigate={() => {}} />
          </NodeCard>
        </div>
      )}
    </>
  );
}
