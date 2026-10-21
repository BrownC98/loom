import { Maximize2, Minimize2, Plus, Trash2 } from 'lucide-react';
import { Fragment, useEffect, useImperativeHandle, useRef, useState, type ReactNode, type Ref } from 'react';
import { useTranslation } from 'react-i18next';
import { applyNodeChanges, Handle, MiniMap, Position, ReactFlow, useReactFlow, useStoreApi, useUpdateNodeInternals, ViewportPortal, type Edge, type FinalConnectionState, type Node, type NodeChange, type NodeProps, type XYPosition } from '@xyflow/react';
import { adoptNode, clearSpots, getSpots, nodePty, putSpots, removeNode } from './api';
import { agentLabel, recentAgent } from './catalog';
import type { AccountView } from './gen/AccountView';
import type { AgentView } from './gen/AgentView';
import type { CardView } from './gen/CardView';
import type { Toward } from './gen/Toward';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { Placed } from './gen/Placed';
import type { RepoView } from './gen/RepoView';
import type { SettingsView } from './gen/SettingsView';
import type { Spot } from './gen/Spot';
import { DeleteWorktreePopover } from './DeleteWorktreePopover';
import { arrangeTree, centerSide, crossed, edgeSides, FREE_GAP, freeSpot, GROUP_GAP, overlaps, placeGroups, push, SIDES, TREE_SIDES, type ArrangeDirection, type Box, type CenterSide, type Side, type Sibling, type Size } from './layout';
import { NewNodePopover } from './NewNodePopover';
import { NodeCard, SimpleCard } from './NodeCard';
import { attachLevels, groupParents, groupsOf, inGroup, type Group, type GroupKind } from './repos';
import { Terminal, type Direction } from './Terminal';

// 배치 값은 승인한 목업(docs/mockups/main.html)의 좌표에서 가져왔다.
const GROUP_POSITION: XYPosition = { x: 16, y: 64 };
/** 끄는 상자의 z. React Flow가 고른 노드에 더하는 1000(`SELECTED_NODE_Z`)과 그 묶음의 카드보다 높아야 한다. */
const DRAGGING_Z = 2000;
/** 빈 묶음을 재기 전에 쓰는 크기(머리와 안내 글). 재면 잰 크기다. 빈 묶음은 카드 폭을 차지하지 않는다(목업 docs/mockups/repo-drag.html 안 A). */
const EMPTY_GROUP: Size = { width: 320, height: 100 };
/** 카드와 묶음 테두리 사이 틈. 왼쪽·오른쪽·아래가 같다. */
const CARD_X = 20;
const CARD_TOP = 60;
/**
 * 카드를 재기 전에 쓰는 크기. 재면 잰 크기로 배치한다. `width`는 styles.css `.card`·`.card.simple`의 폭이다.
 * 표준 높이는 머리, 80×24 터미널(styles.css `.term`), 발 줄의 합이다. 심플 높이는 목업 docs/mockups/card-view.html에서 쟀다.
 */
const CARD_SIZE: Record<CardView, Size> = {
  standard: { width: 600, height: 466 },
  simple: { width: 260, height: 134 },
};
const POSITION: Record<Side, Position> = { top: Position.Top, right: Position.Right, bottom: Position.Bottom, left: Position.Left };
/** [새 노드] 버튼 아래로 띄우는 새 노드 팝오버의 틈. 팝오버 오른쪽 끝을 버튼 오른쪽 끝에 맞춘다. */
const POPOVER_GAP = 6;
/** 묶음 오른쪽 위에서 잰 삭제 경고 위치. 경고는 [워크트리 삭제] 아래에 오른쪽을 맞춰 연다. */
const DELETE_OFFSET: XYPosition = { x: -8, y: 40 };
/** 저장소 상자 테두리와 묶음 사이 틈. `top`은 머리를 포함한다(목업 docs/mockups/repo-scope.html). */
const REPO_INSET = { top: 52, side: 16 };
/** 노드로 옮겨 가는 화면 이동 시간. */
const MOVE_MS = 300;

/** 카드 머리 아이콘 단추(목업 docs/mockups/main.html). 아이콘은 자식으로 받는다. */
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
};
type CardBase = { view: NodeView; agentLabel: string; selected: boolean; port: Side; onSelect: () => void; onNavigate: (dir: Direction) => void; onRemove: () => void };
/** 표준 카드. `ownsPty`: 이 카드가 터미널을 연다. 확대 창이 열면 카드는 터미널을 그리지 않는다. PTY 크기를 한쪽만 정하게 한다. */
type CardData = CardBase & { parentName: string | null; ownsPty: boolean; onExpand: () => void };
type RepoData = { name: string; path: string };
type GroupNode = Node<GroupData, 'worktree'>;
/** 모든 저장소 범위에서 저장소마다 묶음을 두르는 상자. 머리를 끌어 옮긴다. */
type RepoNode = Node<RepoData, 'repo'>;
type CardNode = Node<CardData, 'card'>;
type SimpleNode = Node<CardBase, 'simple'>;
type FlowNode = RepoNode | GroupNode | CardNode | SimpleNode;

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
  return (
    <section className="repo-box" aria-label={data.name}>
      <div className="repo-head">
        <span className="name">{data.name}</span>
        <span className="sub" translate="no" title={data.path}>
          {data.path}
        </span>
      </div>
    </section>
  );
}

function Card({ data }: NodeProps<CardNode>) {
  const { t } = useTranslation();
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
            <IconButton label={t('node.expand')} title={t('node.expandTitle')} onClick={data.onExpand}>
              <Maximize2 />
            </IconButton>
            <RemoveButton onRemove={data.onRemove} />
          </>
        }
      >
        {data.ownsPty && <Terminal src={nodePty(data.view.id)} focused={data.selected} onFocus={data.onSelect} onNavigate={data.onNavigate} />}
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

const nodeTypes = { repo: Repo, worktree: Group, card: Card, simple: Simple };

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
function flowId(spot: Spot): string {
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

/** 강조하는 동안 한 간선으로 오간 메시지 수. 가는 쪽마다 센다. */
export type Pulse = Readonly<Record<Toward, number>>;

/** `neighbor`: 그래프 밖(오른쪽 칸의 터미널)에서 옆 카드를 고를 때 쓴다. `arrange`: 끌어 놓은 자리를 지우고 자동 배치로 되돌린다(오른쪽 사이드바의 [정렬]). */
export type GraphHandle = { neighbor: (from: NodeId, dir: Direction) => NodeId | null; arrange: () => void };

/**
 * `cardView`: 그래프 전체의 카드 보기. 심플이면 카드에 터미널이 없고, 터미널은 부르는 쪽(오른쪽 칸)이 연다.
 * `pulses`: 자식 id → 강조하는 동안 그 간선으로 오간 메시지 수(가는 쪽마다). `agents`·`settings`·`accounts`는 카드 이름과 새 노드 창에 쓴다.
 * `selected`: 입력을 받는 카드. 트리도 고르므로 부르는 쪽이 가진다.
 * `highlight`: 이벤트 콘솔에서 고른 이벤트에 관여한 노드. 그 노드와 그 사이 간선을 강조하고 나머지는 흐리게 한다. 바탕을 누르면 끈다.
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
  highlight: ReadonlySet<NodeId> | null;
  onClearHighlight: () => void;
  cardView: CardView;
  /** 나무가 뻗는 쪽. 오른쪽 사이드바의 [정렬] 메뉴가 고른다. */
  direction: ArrangeDirection;
  ref?: Ref<GraphHandle>;
}) {
  const { t } = useTranslation();
  const { highlight } = props;
  /** 강조 중이면 `ids`가 모두 관여한 쪽은 `hl`, 아니면 `dim`. */
  const lit = (...ids: NodeId[]) => (highlight === null ? undefined : ids.every((id) => highlight.has(id)) ? 'hl' : 'dim');
  const { selected, onSelect: setSelected } = props;
  const flow = useReactFlow();
  const [expanded, setExpanded] = useState<NodeId | null>(null);
  // 새 노드 창이나 삭제 경고를 연 묶음(워크트리 경로).
  const [creating, setCreating] = useState<string | null>(null);
  // [새 노드] 버튼의 오른쪽 아래 모서리(묶음 왼쪽 위에서 잰 자리). 창을 그 자리에 오른쪽을 맞춰 띄운다. 묶음을 끌면 함께 움직인다.
  const [anchor, setAnchor] = useState<XYPosition | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
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
  useEffect(() => {
    getSpots()
      .then((all) => setSpots(new Map(all.map((p) => [flowId(p.spot), p]))))
      .catch((err: Error) => setRefusal(err.message));
  }, []);
  const { direction } = props;
  const placedAt = (key: string): XYPosition | undefined => moving.get(key) ?? spots.get(key)?.at;

  const views = [...props.views.values()];
  const estimate = CARD_SIZE[props.cardView];
  /** 잰 카드 크기. 재기 전에는 보기마다 정한 크기다. */
  const size = (id: NodeId): Size => {
    const measured = flowState.get(String(id))?.measured;
    return measured?.width && measured.height ? { width: measured.width, height: measured.height } : estimate;
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
  const layGroup = (group: Group, members: NodeView[], step: number) => {
    const tree = arrangeTree(members, size, direction, step);
    // 나무 배치는 카드 자리 기준이라 `CARD_X`·`CARD_TOP`만큼 옮긴다. 가장 왼쪽·위 카드를 그 틈에 맞춘다.
    const min = { x: Math.min(...[...tree.values()].map((p) => p.x)), y: Math.min(...[...tree.values()].map((p) => p.y)) };
    const local = settle(
      members.map((v) => ({ key: String(v.id), size: size(v.id), lead: { x: 0, y: 0 }, auto: { x: CARD_X + tree.get(v.id)!.x - min.x, y: CARD_TOP + tree.get(v.id)!.y - min.y } })),
    );
    for (const v of members) spotOf.set(String(v.id), { kind: 'node', repo: v.place.repo, node: v.id });
    share({ boxes: members.map((v) => ({ key: String(v.id), at: local.get(String(v.id))!, size: size(v.id) })), lead: new Map(), parent: group.path });
    // 묶음은 카드를 모두 덮는다(네 쪽). 빈 묶음은 머리와 안내 글 크기다(높이·폭을 비워 내용에 맞춘다).
    const measured = flowState.get(group.path)?.measured;
    const box =
      members.length > 0
        ? cover(members.map((v) => ({ at: local.get(String(v.id))!, size: size(v.id) })), { top: CARD_TOP, side: CARD_X })
        : { at: { x: 0, y: 0 }, size: { width: measured?.width ?? EMPTY_GROUP.width, height: measured?.height ?? EMPTY_GROUP.height } };
    return { group, members, local, lead: box.at, size: box.size };
  };
  // 저장소가 둘 이상이면(모든 저장소 범위) 저장소마다 상자를 두르고 그 안에 묶음을 놓는다(목업 docs/mockups/repo-scope.html).
  const framed = props.repos.length > 1;
  const repos = props.repos.map((repo) => {
    const repoViews = views.filter((v) => v.place.repo === repo.id);
    const repoGroups = groupsOf(repo, repoViews);
    // 한 단계는 가장 큰 카드에 묶음 머리·테두리와 묶음 틈을 더한 크기다. 그래서 깊이가 같은 카드는 묶음과 관계없이 같은 줄에 선다(목업 docs/mockups/same-worktree.html 안 B).
    const vertical = direction === 'down' || direction === 'up';
    const along = (s: Size) => (vertical ? s.height : s.width);
    const step = (repoViews.length > 0 ? Math.max(...repoViews.map((v) => along(size(v.id)))) : along(estimate)) + (vertical ? CARD_TOP + CARD_X : 2 * CARD_X) + GROUP_GAP;
    const laid = repoGroups.map((group) => layGroup(group, inGroup(repoViews, group), step));
    // 자동 배치는 부모 카드 다음 단계에 놓는 나무다. 자리는 저장소 원점 기준 묶음 상자 왼쪽 위다.
    const parents = groupParents(repoGroups, repoViews);
    const starts = attachLevels(repoGroups, repoViews, parents);
    const auto = placeGroups(
      laid.map((g, i) => ({ size: g.size, parent: parents[i], start: starts[i] })),
      direction,
      { x: 0, y: 0 },
      step,
    );
    const at = settle(laid.map((g, i) => ({ key: g.group.path, size: g.size, lead: g.lead, auto: auto[i] })));
    for (const g of laid) spotOf.set(g.group.path, { kind: 'worktree', repo: repo.id, path: g.group.path });
    const key = `repo:${repo.id}`;
    share({ boxes: laid.map((g) => ({ key: g.group.path, at: at.get(g.group.path)!, size: g.size })), lead: new Map(laid.map((g) => [g.group.path, g.lead])), parent: framed ? key : null });
    // 저장소 상자는 묶음을 모두 덮는다(네 쪽). 끄는 카드로 묶음이 넓어지면 함께 넓어진다.
    const box = cover(laid.map((g) => ({ at: at.get(g.group.path)!, size: g.size })), REPO_INSET);
    return { repo, key, laid: laid.map((g) => ({ ...g, at: at.get(g.group.path)! })), lead: box.at, size: box.size };
  });
  // 저장소 상자도 같은 규칙이다. 자동 배치는 모두 뿌리라 사이드바 순서로 옆에 늘어선다.
  const repoAuto = placeGroups(
    repos.map((r) => ({ size: r.size, parent: null, start: 1 })),
    direction,
    GROUP_POSITION,
    Math.max(...repos.map((r) => (direction === 'down' || direction === 'up' ? r.size.height : r.size.width))) + GROUP_GAP,
  );
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
  const navigate = (from: NodeId) => (dir: Direction) => {
    const to = neighbor(positions, from, dir);
    if (to !== null) setSelected(to);
  };
  useImperativeHandle(props.ref, () => ({
    neighbor: (from, dir) => neighbor(positions, from, dir),
    // [정렬]: 보이는 저장소의 자리를 지운다. 지우지 못한 저장소는 다음에 다시 읽으면 저장한 자리로 돌아간다.
    arrange: () => {
      const shown = new Set(props.repos.map((r) => r.id));
      setMoving(new Map());
      setSpots((prev) => new Map([...prev].filter(([, p]) => !shown.has(p.spot.repo))));
      for (const id of shown) clearSpots(id).catch((err: Error) => setRefusal(err.message));
    },
  }));
  const standard = props.cardView === 'standard';
  // 끄는 상자는 React Flow가 옮긴 자리에 그린다. React Flow가 그 노드에 `dragging` 클래스를 붙여 반투명하게 보인다.
  // 끄는 카드는 계산한 자리다. React Flow 자리는 넓어지기 전 묶음 기준이라 묶음이 넓어지면 어긋난다.
  // 늘 맨 위다(2026-10-20 사람 지시 "드래그로 잡은건 겹칠때 항상 최상위 z 인덱스 적용"). 묶음을 끌면 그 카드도 함께 올라온다.
  const keep = <N extends FlowNode>(node: N): N => {
    const live = flowState.get(node.id);
    return { ...node, measured: live?.measured, ...(live?.dragging && { position: node.parentId ? node.position : live.position, dragging: true, zIndex: DRAGGING_Z }) };
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
          dragHandle: '.repo-head',
          data: { name: r.repo.name, path: r.repo.path },
        }),
      )
    : [];
  const nodes: FlowNode[] = [...repoNodes, ...groups.flatMap(({ group, members, local, lead, origin, size: box }) => [
    keep<GroupNode>({
      id: group.path,
      type: 'worktree',
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
        className: lit(view.id),
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
      return keep<CardNode>({
        ...base,
        type: 'card',
        dragHandle: '.card-head',
        data: {
          ...data,
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
    const sides = from && to ? edgeSides({ at: from, size: size(view.parent) }, { at: to, size: size(view.id) }, direction) : TREE_SIDES[direction];
    return [
      {
        id: `edge-${view.id}`,
        source: String(view.parent),
        target: String(view.id),
        sourceHandle: sides.parent,
        targetHandle: sides.child,
        className: [pulse ? 'live' : undefined, flow, lit(view.id, view.parent)].filter(Boolean).join(' ') || undefined,
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
    const done = new Map(moving);
    done.set(key, originOf(level, key, dragged.at));
    let [at, mover, lv] = [key, dragged, level];
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
      const contents = boxes.map((b) => (pushed.has(b.key) ? { ...b, at: pushed.get(b.key)! } : b.key === at ? { ...b, at: mover.at } : b));
      const shell = cover(contents, spotOf.get(lv.parent)?.kind === 'worktree' ? { top: CARD_TOP, side: CARD_X } : REPO_INSET);
      const self = outer.boxes.find((b) => b.key === lv.parent)!;
      const origin = done.get(self.key) ?? originOf(outer, self.key, self.at);
      [at, mover, lv] = [self.key, { at: { x: origin.x + shell.at.x, y: origin.y + shell.at.y }, size: shell.size }, outer];
    }
    setMoving(new Map());
    setSpots((prev) => new Map([...prev, ...[...done].map(([k, p]): [string, Placed] => [k, { spot: spotOf.get(k)!, at: p }])]));
    putSpots([...done].map(([k, p]) => ({ spot: spotOf.get(k)!, at: p }))).catch((err: Error) => setRefusal(err.message));
  };

  const onNodesChange = (changes: NodeChange<FlowNode>[]) => {
    const next = new Map(applyNodeChanges(changes, nodes).map((n) => [n.id, n]));
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
        onNodesChange={onNodesChange}
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
        elementsSelectable={false}
        nodesFocusable={false}
        deleteKeyCode={null}
        zoomOnDoubleClick={false}
        minZoom={0.4}
        maxZoom={1}
      >
        <MiniMap
          pannable
          ariaLabel={t('graph.minimap')}
          nodeClassName={(node) => (node.type === 'repo' || node.type === 'worktree' ? node.type : (node as CardNode | SimpleNode).data.view.state.kind)}
          onClick={(_, position) => flow.setCenter(position.x, position.y, { zoom: flow.getZoom(), duration: MOVE_MS })}
        />
        {groups.map(({ group, repo, members, origin, right }) => {
          const at = creating === group.path && anchor ? { x: origin.x + anchor.x, y: origin.y + anchor.y } : null;
          return (
            // 묶음 노드 안에 두면 자식 카드가 팝오버를 덮는다. 뷰포트 층에 그려 카드 위에 둔다.
            (creating === group.path || deleting === group.path) && (
              <ViewportPortal key={group.path}>
                <div
                  className="pop-end"
                  style={{
                    position: 'absolute',
                    // 새 노드 창은 누른 [새 노드] 버튼의 오른쪽 아래 모서리에 오른쪽을 맞춰 붙인다.
                    transform: at ? `translate(${at.x}px, ${at.y}px)` : `translate(${origin.x + right + DELETE_OFFSET.x}px, ${origin.y + DELETE_OFFSET.y}px)`,
                  }}
                >
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
                </div>
              </ViewportPortal>
            )
          );
        })}
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
