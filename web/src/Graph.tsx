import { useEffect, useImperativeHandle, useRef, useState, type Ref } from 'react';
import { useTranslation } from 'react-i18next';
import { applyNodeChanges, Handle, MiniMap, Position, ReactFlow, useReactFlow, useStoreApi, useUpdateNodeInternals, ViewportPortal, type CoordinateExtent, type Edge, type FinalConnectionState, type Node, type NodeChange, type NodeProps, type XYPosition } from '@xyflow/react';
import { adoptNode, nodePty, removeNode } from './api';
import { agentLabel, recentAgent } from './catalog';
import type { AccountView } from './gen/AccountView';
import type { AgentView } from './gen/AgentView';
import type { CardView } from './gen/CardView';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { RepoView } from './gen/RepoView';
import type { SettingsView } from './gen/SettingsView';
import { DeleteWorktreePopover } from './DeleteWorktreePopover';
import { arrangeTree, placeGroups, type ArrangeDirection, type Size } from './layout';
import { NewNodePopover } from './NewNodePopover';
import { NodeCard, SimpleCard } from './NodeCard';
import { groupsOf, inGroup, type GroupKind } from './repos';
import { Terminal, type Direction } from './Terminal';

// 배치 값은 승인한 목업(docs/mockups/main.html)의 좌표에서 가져왔다.
const GROUP_POSITION: XYPosition = { x: 16, y: 64 };
/** 빈 묶음을 재기 전에 쓰는 높이(머리와 안내 글). */
const EMPTY_GROUP_HEIGHT = 100;
/** 카드와 묶음 테두리 사이 틈. 왼쪽·오른쪽·아래가 같다. */
const CARD_X = 20;
const CARD_TOP = 60;
/** 묶음의 가장 작은 폭. 머리(이름, 버튼 둘)가 한 줄에 들어간다(목업 docs/mockups/card-view.html). 긴 이름은 말줄임한다. */
const GROUP_MIN_WIDTH = 400;
/**
 * 카드를 재기 전에 쓰는 크기. 재면 잰 크기로 배치한다. `width`는 styles.css `.card`·`.card.simple`의 폭이다.
 * 표준 높이는 머리, 80×24 터미널(styles.css `.term`), 발 줄의 합이다. 심플 높이는 목업 docs/mockups/card-view.html에서 쟀다.
 */
const CARD_SIZE: Record<CardView, Size> = {
  standard: { width: 600, height: 466 },
  simple: { width: 260, height: 134 },
};
/** 간선이 붙는 쪽. 부모 쪽(`target`)과 자식 쪽(`source`)이 나무가 뻗는 쪽을 따른다. */
const HANDLES: Record<ArrangeDirection, { target: Position; source: Position }> = {
  down: { target: Position.Top, source: Position.Bottom },
  up: { target: Position.Bottom, source: Position.Top },
  right: { target: Position.Left, source: Position.Right },
  left: { target: Position.Right, source: Position.Left },
};
/** 카드는 묶음 머리 위나 왼쪽 테두리 밖으로 끌 수 없다. 오른쪽·아래로 끌면 묶음이 따라 커진다. 좌표는 묶음 기준이다. */
const CARD_EXTENT: CoordinateExtent = [
  [0, CARD_TOP],
  [Infinity, Infinity],
];
/** [새 노드] 버튼 아래로 띄우는 새 노드 팝오버의 틈. 팝오버 오른쪽 끝을 버튼 오른쪽 끝에 맞춘다. */
const POPOVER_GAP = 6;
/** 묶음 오른쪽 위에서 잰 삭제 경고 위치. 경고는 [워크트리 삭제] 아래에 오른쪽을 맞춰 연다. */
const DELETE_OFFSET: XYPosition = { x: -8, y: 40 };
/** 노드로 옮겨 가는 화면 이동 시간. */
const MOVE_MS = 300;

/** 카드 머리 아이콘(목업 docs/mockups/main.html). */
const ICON = {
  expand: 'M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7',
  collapse: 'M4 14h6v6M20 10h-6V4M14 10l7-7M3 21l7-7',
  remove: 'M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3',
};

function IconButton(props: { label: string; title: string; path: string; onClick: () => void }) {
  return (
    <button type="button" className="btn ghost sm icon nodrag" aria-label={props.label} title={props.title} onClick={props.onClick}>
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path d={props.path} />
      </svg>
    </button>
  );
}

/** 그래프 화면을 `ids` 노드가 모두 보이게 옮긴다. 그래프 밖(트리)에서 노드로 옮겨 갈 때 쓴다. `ReactFlowProvider` 안에서만 부른다. */
export function useShowNodes(): (ids: NodeId[]) => void {
  const flow = useReactFlow();
  return (ids) => flow.fitView({ nodes: ids.map((id) => ({ id: String(id) })), duration: MOVE_MS, maxZoom: 1 });
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
type CardBase = { view: NodeView; agentLabel: string; selected: boolean; handles: (typeof HANDLES)[ArrangeDirection]; onSelect: () => void; onNavigate: (dir: Direction) => void; onRemove: () => void };
/** 표준 카드. `ownsPty`: 이 카드가 터미널을 연다. 확대 창이 열면 카드는 터미널을 그리지 않는다. PTY 크기를 한쪽만 정하게 한다. */
type CardData = CardBase & { parentName: string | null; ownsPty: boolean; onExpand: () => void };
type GroupNode = Node<GroupData, 'worktree'>;
type CardNode = Node<CardData, 'card'>;
type SimpleNode = Node<CardBase, 'simple'>;
type FlowNode = GroupNode | CardNode | SimpleNode;

function Group({ data }: NodeProps<GroupNode>) {
  const { t } = useTranslation();
  return (
    <section className="group" aria-label={t('graph.group', { branch: data.branch })}>
      <div className="group-head">
        <span className="branch" translate="no">
          {data.branch}
        </span>
        {data.kind !== 'linked' && <span className="kind">{t(`worktree.${data.kind}`)}</span>}
        <span className="spacer" />
        <button className="btn ghost sm nodrag" aria-expanded={data.creating} onClick={(e) => data.onOpen(e.currentTarget)}>
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M12 5v14M5 12h14" />
          </svg>
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
            <IconButton label={t('node.expand')} title={t('node.expandTitle')} path={ICON.expand} onClick={data.onExpand} />
            <RemoveButton onRemove={data.onRemove} />
          </>
        }
      >
        {data.ownsPty && <Terminal src={nodePty(data.view.id)} focused={data.selected} onFocus={data.onSelect} onNavigate={data.onNavigate} />}
      </NodeCard>
      <Ports handles={data.handles} />
    </>
  );
}

function Simple({ data }: NodeProps<SimpleNode>) {
  return (
    <>
      <SimpleCard view={data.view} agentLabel={data.agentLabel} selected={data.selected} onSelect={data.onSelect} onNavigate={data.onNavigate} actions={<RemoveButton onRemove={data.onRemove} />} />
      <Ports handles={data.handles} />
    </>
  );
}

function RemoveButton(props: { onRemove: () => void }) {
  const { t } = useTranslation();
  return <IconButton label={t('node.remove')} title={t('node.remove')} path={ICON.remove} onClick={props.onRemove} />;
}

/** 간선이 붙는 자리. 자식 쪽 잇기 점을 끌어 다른 카드에 놓으면 그 카드의 부모가 된다(onConnectEnd). */
function Ports(props: { handles: (typeof HANDLES)[ArrangeDirection] }) {
  return (
    <>
      <Handle type="target" position={props.handles.target} isConnectable={false} />
      <Handle type="source" position={props.handles.source} className="port" />
    </>
  );
}

const nodeTypes = { worktree: Group, card: Card, simple: Simple };

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

/** `neighbor`: 그래프 밖(오른쪽 칸의 터미널)에서 옆 카드를 고를 때 쓴다. `arrange`: 끈 카드와 묶음을 모두 배치 자리로 되돌린다(위 막대의 [정렬]). */
export type GraphHandle = { neighbor: (from: NodeId, dir: Direction) => NodeId | null; arrange: () => void };

/**
 * `cardView`: 그래프 전체의 카드 보기. 심플이면 카드에 터미널이 없고, 터미널은 부르는 쪽(오른쪽 칸)이 연다.
 * `pulses`: 자식 id → 강조하는 동안 그 간선으로 오간 메시지 수. `agents`·`settings`·`accounts`는 카드 이름과 새 노드 창에 쓴다.
 * `selected`: 입력을 받는 카드. 트리도 고르므로 부르는 쪽이 가진다.
 * `highlight`: 이벤트 콘솔에서 고른 이벤트에 관여한 노드. 그 노드와 그 사이 간선을 강조하고 나머지는 흐리게 한다. 바탕을 누르면 끈다.
 */
export function Graph(props: {
  /** 그래프가 그리는 저장소. `views`는 이 저장소의 노드만이다. */
  repo: RepoView;
  onRepo: (repo: RepoView) => void;
  views: ReadonlyMap<NodeId, NodeView>;
  pulses: ReadonlyMap<NodeId, number>;
  agents: AgentView[];
  settings: SettingsView;
  accounts: AccountView[] | null;
  selected: NodeId | null;
  onSelect: (id: NodeId | null) => void;
  highlight: ReadonlySet<NodeId> | null;
  onClearHighlight: () => void;
  cardView: CardView;
  /** 나무가 뻗는 쪽. 위 막대의 [정렬] 메뉴가 고른다. */
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
  // 사람이 끈 카드와 묶음의 위치만 둔다(React Flow 노드 id 기준). 나머지는 늘 배치를 따르므로, 노드를 지우면 빈자리를 채운다.
  const [dragged, setDragged] = useState<ReadonlyMap<string, XYPosition>>(new Map());
  const { direction } = props;

  const views = [...props.views.values()];
  const estimate = CARD_SIZE[props.cardView];
  /** 잰 카드 크기. 재기 전에는 보기마다 정한 크기다. */
  const size = (id: NodeId): Size => {
    const measured = flowState.get(String(id))?.measured;
    return measured?.width && measured.height ? { width: measured.width, height: measured.height } : estimate;
  };
  // 묶음마다 나무 배치를 따로 한다. 카드 위치는 묶음 기준, `positions`는 키보드 이동에 쓰는 화면 기준이다.
  const positions = new Map<NodeId, XYPosition>();
  const laid = groupsOf(props.repo, views).map((group) => {
    const members = inGroup(views, group);
    const tree = arrangeTree(members, size, direction);
    const local = new Map<NodeId, XYPosition>(
      members.map((v) => {
        const at = tree.get(v.id)!;
        return [v.id, dragged.get(String(v.id)) ?? { x: CARD_X + at.x, y: CARD_TOP + at.y }];
      }),
    );
    // 묶음은 가장 오른쪽·아래 카드까지만 덮는다. 빈 묶음은 머리와 안내 글만큼이다(높이를 비워 내용에 맞춘다).
    const right = Math.max(GROUP_MIN_WIDTH, Math.max(CARD_X + estimate.width, ...members.map((v) => local.get(v.id)!.x + size(v.id).width)) + CARD_X);
    const bottom = Math.max(...members.map((v) => local.get(v.id)!.y + size(v.id).height)) + CARD_X;
    const height = members.length > 0 ? bottom : (flowState.get(group.path)?.measured?.height ?? EMPTY_GROUP_HEIGHT);
    return { group, members, local, right, bottom, height };
  });
  // 묶음끼리는 정렬 방향을 따라 한 줄로 놓고, 커지거나 끈 묶음이 겹치면 민다.
  const origins = placeGroups(
    laid.map((g) => ({ size: { width: g.right, height: g.height }, dragged: dragged.get(g.group.path) })),
    direction,
    GROUP_POSITION,
  );
  const groups = laid.map((g, i) => {
    const origin = origins[i];
    for (const [id, p] of g.local) positions.set(id, { x: origin.x + p.x, y: origin.y + p.y });
    return { ...g, origin };
  });
  // 묶음이 생기면(방금 만든 워크트리) 화면을 그 묶음으로, 빠지면 첫 묶음으로 옮긴다. 묶음은 오른쪽에 붙어 화면 밖에 생기기 쉽다.
  const known = useRef<readonly string[] | null>(null);
  const groupKey = groups.map((g) => g.group.path).join('\n');
  useEffect(() => {
    const paths = groupKey.split('\n');
    const before = known.current;
    known.current = paths;
    if (before === null) return;
    const target = paths.find((path) => !before.includes(path)) ?? (before.some((path) => !paths.includes(path)) ? paths[0] : undefined);
    if (target) void flow.fitView({ nodes: [{ id: target }], duration: MOVE_MS, maxZoom: 1 });
  }, [groupKey, flow]);
  const navigate = (from: NodeId) => (dir: Direction) => {
    const to = neighbor(positions, from, dir);
    if (to !== null) setSelected(to);
  };
  useImperativeHandle(props.ref, () => ({ neighbor: (from, dir) => neighbor(positions, from, dir), arrange: () => setDragged(new Map()) }));
  const standard = props.cardView === 'standard';
  const keep = <N extends FlowNode>(node: N): N => ({ ...node, measured: flowState.get(node.id)?.measured });

  const nodes: FlowNode[] = groups.flatMap(({ group, members, local, origin, right, bottom }) => [
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
        extent: CARD_EXTENT,
        className: lit(view.id),
        position: local.get(view.id)!,
      };
      const data: CardBase = {
        view,
        agentLabel: agentLabel(props.agents, view.agent),
        selected: selected === view.id,
        handles: HANDLES[direction],
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
  ]);

  const edges: Edge[] = views.flatMap((view) => {
    if (view.parent === null) return [];
    const messages = props.pulses.get(view.id);
    return [
      {
        id: `edge-${view.id}`,
        source: String(view.parent),
        target: String(view.id),
        className: [messages ? 'live' : undefined, lit(view.id, view.parent)].filter(Boolean).join(' ') || undefined,
        label: messages ? t('graph.messages', { n: messages }) : undefined,
        labelBgPadding: [8, 2],
        labelBgBorderRadius: 10,
        focusable: false,
      },
    ];
  });

  const onNodesChange = (changes: NodeChange<FlowNode>[]) => {
    setFlowState(new Map(applyNodeChanges(changes, nodes).map((n) => [n.id, n])));
    // 카드나 묶음을 끌 때만 위치 변경이 온다(키보드 이동은 껐다).
    const moved = changes.flatMap((c) => (c.type === 'position' && c.position ? [[c.id, c.position] as const] : []));
    if (moved.length > 0) setDragged((prev) => new Map([...prev, ...moved]));
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
  // 정렬 방향이 바뀌면 간선이 붙는 쪽(`HANDLES`)이 바뀐다. React Flow는 핸들 자리를 다시 재야 간선을 옮긴다.
  const cardIds = views.map((v) => String(v.id)).join(',');
  useEffect(() => {
    if (cardIds) remeasure(cardIds.split(','));
  }, [direction, cardIds, remeasure]);

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
        onNodeClick={(_, node) => node.type !== 'worktree' && setSelected(Number(node.id))}
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
          nodeClassName={(node) => (node.type === 'worktree' ? 'worktree' : (node as CardNode | SimpleNode).data.view.state.kind)}
          onClick={(_, position) => flow.setCenter(position.x, position.y, { zoom: flow.getZoom(), duration: MOVE_MS })}
        />
        {groups.map(({ group, members, origin, right }) => {
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
                      place={{ repo: props.repo.id, worktree: group.path }}
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
                      repo={props.repo.id}
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
            headAction={<IconButton label={t('node.collapse')} title={t('node.collapse')} path={ICON.collapse} onClick={() => setExpanded(null)} />}
          >
            <Terminal src={nodePty(zoomedView.id)} focused={selected === zoomedView.id} onFocus={() => setSelected(zoomedView.id)} onNavigate={() => {}} />
          </NodeCard>
        </div>
      )}
    </>
  );
}
