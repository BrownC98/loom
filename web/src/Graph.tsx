import { Maximize2, Minimize2, Plus, Trash2 } from 'lucide-react';
import { Fragment, useEffect, useImperativeHandle, useState, type ReactNode, type Ref } from 'react';
import { useTranslation } from 'react-i18next';
import { applyNodeChanges, Handle, MiniMap, Position, ReactFlow, useReactFlow, useStoreApi, useUpdateNodeInternals, ViewportPortal, type Edge, type FinalConnectionState, type Node, type NodeChange, type NodeProps, type XYPosition } from '@xyflow/react';
import { adoptNode, nodePty, removeNode } from './api';
import { agentLabel, recentAgent } from './catalog';
import type { AccountView } from './gen/AccountView';
import type { AgentView } from './gen/AgentView';
import type { CardView } from './gen/CardView';
import type { Toward } from './gen/Toward';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { RepoView } from './gen/RepoView';
import type { SettingsView } from './gen/SettingsView';
import { DeleteWorktreePopover } from './DeleteWorktreePopover';
import { arrangeTree, edgeSides, growBox, placeGroups, reorder, SIDES, TREE_SIDES, type ArrangeDirection, type Side, type Sibling, type Size } from './layout';
import { NewNodePopover } from './NewNodePopover';
import { NodeCard, SimpleCard } from './NodeCard';
import { groupParents, groupsOf, inGroup, type Group, type GroupKind } from './repos';
import { Terminal, type Direction } from './Terminal';

// 배치 값은 승인한 목업(docs/mockups/main.html)의 좌표에서 가져왔다.
const GROUP_POSITION: XYPosition = { x: 16, y: 64 };
/** 빈 묶음을 재기 전에 쓰는 높이(머리와 안내 글). */
/** 끄는 상자의 z. React Flow가 고른 노드에 더하는 1000(`SELECTED_NODE_Z`)과 그 묶음의 카드보다 높아야 한다. */
const DRAGGING_Z = 2000;
const EMPTY_GROUP_HEIGHT = 100;
/** 카드와 묶음 테두리 사이 틈. 왼쪽·오른쪽·아래가 같다. */
const CARD_X = 20;
const CARD_TOP = 60;
const CARD_INSET = { top: CARD_TOP, side: CARD_X };
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
/** 모든 저장소 범위에서 저장소마다 묶음을 두르는 상자. 끌지 않는다. 순서는 사이드바 순서다. */
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

/** 강조하는 동안 한 간선으로 오간 메시지 수. 가는 쪽마다 센다. */
export type Pulse = Readonly<Record<Toward, number>>;

/** `neighbor`: 그래프 밖(오른쪽 칸의 터미널)에서 옆 카드를 고를 때 쓴다. `arrange`: 끌어 바꾼 순서를 버리고 만든 순서로 되돌린다(오른쪽 사이드바의 [정렬]). */
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
  // 사람이 끌어 바꾼 순서(React Flow 노드 id). 앞에 있을수록 형제 사이에서 앞이다. 없는 상자는 그 뒤에 만든 순서로 선다.
  // 끈 형제 묶음을 통째로 끝에 다시 적으므로 다른 형제 묶음의 순서는 그대로다.
  const [order, setOrder] = useState<readonly string[]>([]);
  // 끄는 카드의 자리(묶음이 넓어지기 전 묶음 왼쪽 위 기준). 묶음은 이 카드를 담게 넓어진다. 놓으면 비운다.
  const [drag, setDrag] = useState<{ card: NodeId; at: XYPosition } | null>(null);
  const { direction } = props;

  const views = [...props.views.values()];
  const estimate = CARD_SIZE[props.cardView];
  /** 잰 카드 크기. 재기 전에는 보기마다 정한 크기다. */
  const size = (id: NodeId): Size => {
    const measured = flowState.get(String(id))?.measured;
    return measured?.width && measured.height ? { width: measured.width, height: measured.height } : estimate;
  };
  const rank = (key: string) => {
    const i = order.indexOf(key);
    return i < 0 ? Infinity : i;
  };
  // 끄는 상자 id → 그 형제(끄는 상자 포함, 배치 자리). 끄는 동안 `reorder`에 넘긴다.
  // 카드의 형제는 같은 묶음에서 부모가 같은 카드다. 묶음의 형제는 부모 묶음이 같은 묶음이다.
  const siblings = new Map<string, Sibling[]>();
  const share = (boxes: readonly (Sibling & { parent: unknown })[]) => {
    for (const b of boxes) siblings.set(b.key, boxes.filter((o) => o.parent === b.parent));
  };
  // 묶음마다 나무 배치를 따로 한다. 카드 위치는 묶음 기준, `positions`는 키보드 이동에 쓰는 화면 기준이다.
  const positions = new Map<NodeId, XYPosition>();
  /** 묶음 하나의 카드 배치와 묶음 크기. 자리는 묶음 왼쪽 위 기준이다. */
  const layGroup = (group: Group, members: NodeView[]) => {
    const tree = arrangeTree(members, size, (id) => rank(String(id)), direction);
    // 나무 배치는 카드 자리 기준이라 `CARD_X`·`CARD_TOP`만큼 옮긴다. 가장 왼쪽·위 카드를 그 틈에 맞춰 네 쪽 모두 빈 곳이 없게 한다.
    // `slot`은 넓어지기 전 묶음 기준이다.
    const min = { x: Math.min(...[...tree.values()].map((p) => p.x)), y: Math.min(...[...tree.values()].map((p) => p.y)) };
    const slot = new Map<NodeId, XYPosition>(
      members.map((v) => {
        const at = tree.get(v.id)!;
        return [v.id, { x: CARD_X + at.x - min.x, y: CARD_TOP + at.y - min.y }];
      }),
    );
    const ids = new Set(members.map((v) => v.id));
    share(members.map((v) => ({ key: String(v.id), at: slot.get(v.id)!, size: size(v.id), parent: v.parent !== null && ids.has(v.parent) ? v.parent : null })));
    // 묶음은 가장 오른쪽·아래 카드까지만 덮는다. 빈 묶음은 머리와 안내 글만큼이다(높이를 비워 내용에 맞춘다).
    // 묶음 폭은 카드에 맞춘다. 머리가 좁으면 브랜치 이름을 말줄임한다(2026-10-20 사람 지시 "심플모드일때 워크트리 안에 노드 우측 공백이 생기지 않도록 만들어").
    const fit = {
      width: Math.max(CARD_X + estimate.width, ...members.map((v) => slot.get(v.id)!.x + size(v.id).width)) + CARD_X,
      height: Math.max(...members.map((v) => slot.get(v.id)!.y + size(v.id).height)) + CARD_X,
    };
    const held = drag !== null && ids.has(drag.card) ? drag : null;
    const box = held ? growBox(fit, { at: held.at, size: size(held.card) }, CARD_INSET) : { lead: { x: 0, y: 0 }, size: fit };
    const { lead } = box;
    // 넓어진 묶음 기준. 카드는 화면에서 제자리다. 끄는 카드는 끄는 자리에 그린다.
    const shift = (p: XYPosition) => ({ x: p.x + lead.x, y: p.y + lead.y });
    const local = new Map([...slot].map(([id, p]) => [id, shift(p)]));
    const drawn = held && { card: held.card, at: shift(held.at) };
    const [right, bottom] = [box.size.width, box.size.height];
    const height = members.length > 0 ? bottom : (flowState.get(group.path)?.measured?.height ?? EMPTY_GROUP_HEIGHT);
    return { group, members, local, drawn, lead, right, bottom, height };
  };
  // 저장소가 둘 이상이면(모든 저장소 범위) 저장소마다 상자를 두르고 그 안에 묶음을 놓는다(목업 docs/mockups/repo-scope.html).
  const framed = props.repos.length > 1;
  const start = framed ? { x: REPO_INSET.side, y: REPO_INSET.top } : { x: 0, y: 0 };
  const repos = props.repos.map((repo) => {
    const repoViews = views.filter((v) => v.place.repo === repo.id);
    const repoGroups = groupsOf(repo, repoViews);
    const laid = repoGroups.map((group) => layGroup(group, inGroup(repoViews, group)));
    // 묶음끼리도 부모 묶음 다음 단계에 놓는 나무다. 자리는 저장소 상자 왼쪽 위 기준이다.
    const parents = groupParents(repoGroups, repoViews);
    const at = placeGroups(
      // 왼쪽·위로 넓어진 폭은 자리 밖이다. 묶음 상자는 자리보다 `lead`만큼 왼쪽 위에서 시작한다.
      laid.map((g, i) => ({ size: { width: g.right - g.lead.x, height: g.height - g.lead.y }, parent: parents[i], rank: rank(g.group.path) })),
      direction,
      start,
    ).map((p, i) => ({ x: p.x - laid[i].lead.x, y: p.y - laid[i].lead.y }));
    // 저장소 상자는 가장 오른쪽·아래 묶음까지 덮는다. 끄는 카드로 묶음이 넓어지면 함께 넓어진다.
    const size = {
      width: Math.max(0, ...laid.map((g, i) => at[i].x + g.right)) + REPO_INSET.side,
      height: Math.max(0, ...laid.map((g, i) => at[i].y + g.height)) + REPO_INSET.side,
    };
    return { repo, laid, parents, at, size };
  });
  // 저장소 상자도 같은 나무 배치다. 모두 뿌리라 사이드바 순서로 옆에 늘어선다. 끌어 순서를 바꾸지 않는다.
  const repoOrigins = placeGroups(
    repos.map((r, i) => ({ size: r.size, parent: null, rank: i })),
    direction,
    GROUP_POSITION,
  );
  const groups = repos.flatMap((r, ri) => {
    const laid = r.laid.map((g, i) => ({ ...g, repo: r.repo.id, origin: { x: repoOrigins[ri].x + r.at[i].x, y: repoOrigins[ri].y + r.at[i].y } }));
    // 묶음의 형제는 같은 저장소에서 부모 묶음이 같은 묶음이다.
    share(laid.map((g, i) => ({ key: g.group.path, at: g.origin, size: { width: g.right, height: g.height }, parent: `${r.repo.id}:${r.parents[i]}` })));
    for (const g of laid) for (const [id, p] of g.local) positions.set(id, { x: g.origin.x + p.x, y: g.origin.y + p.y });
    return laid;
  });
  const navigate = (from: NodeId) => (dir: Direction) => {
    const to = neighbor(positions, from, dir);
    if (to !== null) setSelected(to);
  };
  useImperativeHandle(props.ref, () => ({
    neighbor: (from, dir) => neighbor(positions, from, dir),
    arrange: () => setOrder([]),
  }));
  const standard = props.cardView === 'standard';
  // 끄는 상자는 React Flow가 옮긴 자리에 그린다. 그 자리(배치 자리)는 비워 둔다. React Flow가 그 노드에 `dragging` 클래스를 붙여 반투명하게 보인다.
  // 끄는 카드는 `drawn` 자리다. React Flow 자리는 넓어지기 전 묶음 기준이라 묶음이 넓어지면 어긋난다.
  // 늘 맨 위다(2026-10-20 사람 지시 "드래그로 잡은건 겹칠때 항상 최상위 z 인덱스 적용"). 묶음을 끌면 그 카드도 함께 올라온다.
  const keep = <N extends FlowNode>(node: N): N => {
    const live = flowState.get(node.id);
    return { ...node, measured: live?.measured, ...(live?.dragging && { position: node.parentId ? node.position : live.position, dragging: true, zIndex: DRAGGING_Z }) };
  };

  // 저장소 상자를 먼저 넣어 묶음과 카드 아래에 그린다.
  const repoNodes: RepoNode[] = framed
    ? repos.map((r, i) =>
        keep<RepoNode>({
          id: `repo:${r.repo.id}`,
          type: 'repo',
          position: repoOrigins[i],
          // 상자는 클릭을 받지 않는다. 상자 안 빈 곳 클릭은 바탕 클릭이다(고르기를 푼다).
          style: { width: r.size.width, height: r.size.height, pointerEvents: 'none' },
          draggable: false,
          data: { name: r.repo.name, path: r.repo.path },
        }),
      )
    : [];
  const nodes: FlowNode[] = [...repoNodes, ...groups.flatMap(({ group, members, local, drawn, origin, right, bottom }) => [
    keep<GroupNode>({
      id: group.path,
      type: 'worktree',
      position: origin,
      style: { width: right, height: members.length > 0 ? bottom : undefined },
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
        position: drawn?.card === view.id ? drawn.at : local.get(view.id)!,
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

  const onNodesChange = (changes: NodeChange<FlowNode>[]) => {
    const next = new Map(applyNodeChanges(changes, nodes).map((n) => [n.id, n]));
    setFlowState(next);
    // 카드나 묶음을 끌 때만 위치 변경이 온다(키보드 이동은 껐다). 끄는 동안 순서를 바로 바꾼다. 놓으면 끄는 상자가 배치 자리로 간다.
    // 카드 자리는 넓어진 지금 묶음 기준이라 `lead`를 빼 넓어지기 전 묶음 기준으로 바꾼다.
    const leads = new Map(groups.map((g) => [g.group.path, g.lead]));
    for (const c of changes) {
      if (c.type !== 'position') continue;
      const node = next.get(c.id);
      if (!node) continue;
      if (!c.dragging) {
        if (node.parentId) setDrag(null);
        continue;
      }
      const lead = node.parentId ? leads.get(node.parentId) : undefined;
      const at = lead ? { x: node.position.x - lead.x, y: node.position.y - lead.y } : node.position;
      if (node.parentId) setDrag({ card: Number(c.id), at });
      const sibs = siblings.get(c.id) ?? [];
      if (sibs.length < 2) continue;
      const keys = reorder(sibs, { ...sibs.find((b) => b.key === c.id)!, at }, direction);
      if (keys) setOrder((prev) => [...prev.filter((k) => !keys.includes(k)), ...keys]);
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
