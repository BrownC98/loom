import { useState } from 'react';
import { applyNodeChanges, Handle, Position, ReactFlow, ViewportPortal, type Edge, type Node, type NodeChange, type NodeProps, type XYPosition } from '@xyflow/react';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import { NewNodePopover } from './NewNodePopover';
import { NodeCard } from './NodeCard';
import { Terminal, type Direction } from './Terminal';

// 배치 값은 승인한 목업(docs/mockups/main.html)의 좌표에서 가져왔다.
const GROUP_POSITION: XYPosition = { x: 16, y: 64 };
const GROUP_MIN_HEIGHT = 672;
const CARD_X = 20;
const CARD_TOP = 60;
/** 카드 높이에 간선이 지나갈 틈을 더한 세로 간격. */
const CARD_STEP = 328;
/** 카드 폭(400)에 틈을 더한 가로 간격. 한 칸이면 묶음 폭이 목업과 같은 440이다. */
const CARD_COLUMN_STEP = 420;
/** 묶음 왼쪽 위에서 잰 새 노드 팝오버 위치. */
const POPOVER_OFFSET: XYPosition = { x: 184, y: 40 };

/** 원래 체크아웃 묶음. 워크트리 묶음은 5단위에서 더한다. */
const MAIN_GROUP = 'main';

type GroupData = { branch: string; empty: boolean; creating: boolean; onOpen: () => void };
type CardData = { view: NodeView; parentName: string | null; selected: boolean; expanded: boolean; onSelect: () => void; onNavigate: (dir: Direction) => void; onExpand: () => void };
type GroupNode = Node<GroupData, 'worktree'>;
type CardNode = Node<CardData, 'card'>;
type FlowNode = GroupNode | CardNode;

function Group({ data }: NodeProps<GroupNode>) {
  return (
    <section className="group" aria-label={`묶음 ${data.branch}`}>
      <div className="group-head">
        <span className="branch" translate="no">
          {data.branch}
        </span>
        <span className="kind">원래 체크아웃</span>
        <span className="spacer" />
        <button className="btn ghost sm" aria-expanded={data.creating} onClick={data.onOpen}>
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M12 5v14M5 12h14" />
          </svg>
          새 노드
        </button>
      </div>
      {data.empty && <p className="group-empty">[새 노드]로 첫 에이전트를 띄운다.</p>}
    </section>
  );
}

function Card({ data }: NodeProps<CardNode>) {
  return (
    <>
      <NodeCard view={data.view} parentName={data.parentName} selected={data.selected} onSelect={data.onSelect} onNavigate={data.onNavigate} onDoubleClickHead={data.onExpand}>
        {/* 확대한 동안에는 확대 창이 터미널을 연다. PTY 크기를 한쪽만 정하게 한다. */}
        {!data.expanded && <Terminal id={data.view.id} focused={data.selected} onFocus={data.onSelect} onNavigate={data.onNavigate} />}
      </NodeCard>
      {/* 간선이 붙는 자리. 사람이 간선을 잇지 않으므로 보이지 않게 둔다. */}
      <Handle type="target" position={Position.Top} isConnectable={false} />
      <Handle type="source" position={Position.Bottom} isConnectable={false} />
    </>
  );
}

const nodeTypes = { worktree: Group, card: Card };

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

type Slot = { column: number; row: number };

/**
 * 나무 배치: 자식은 부모 아래 줄에, 형제는 오른쪽 칸에 놓는다. 잎마다 새 칸을 쓰고 부모는 첫 자식의 칸에 둔다.
 * 사람이 띄운 노드(뿌리)는 앞 뿌리의 나무 아래에 쌓는다. 형제는 만든 순서다.
 */
function treeLayout(views: ReadonlyMap<NodeId, NodeView>): { slots: Map<NodeId, Slot>; columns: number } {
  const children = new Map<NodeId | null, NodeView[]>();
  for (const view of [...views.values()].sort((a, b) => a.id - b.id)) {
    children.set(view.parent, [...(children.get(view.parent) ?? []), view]);
  }
  const slots = new Map<NodeId, Slot>();
  let columns = 1;
  let firstRow = 0;
  for (const root of children.get(null) ?? []) {
    let column = 0;
    let lastRow = firstRow;
    const place = (view: NodeView, row: number) => {
      slots.set(view.id, { column, row });
      lastRow = Math.max(lastRow, row);
      const kids = children.get(view.id) ?? [];
      if (kids.length === 0) column++;
      for (const kid of kids) place(kid, row + 1);
    };
    place(root, firstRow);
    columns = Math.max(columns, column);
    firstRow = lastRow + 1;
  }
  return { slots, columns };
}

function parentName(views: ReadonlyMap<NodeId, NodeView>, view: NodeView): string | null {
  return view.parent === null ? null : (views.get(view.parent)?.name ?? null);
}

/** `pulses`: 자식 id → 강조하는 동안 그 간선으로 오간 메시지 수. */
export function Graph(props: { views: ReadonlyMap<NodeId, NodeView>; pulses: ReadonlyMap<NodeId, number> }) {
  const [selected, setSelected] = useState<NodeId | null>(null);
  const [expanded, setExpanded] = useState<NodeId | null>(null);
  const [creating, setCreating] = useState(false);
  // React Flow가 잰 크기와 끈 위치. 노드를 새 객체로 넘겨도 이 값을 이어받아야 React Flow가 노드를 다시 숨기고 재지 않는다.
  const [flowState, setFlowState] = useState<ReadonlyMap<string, FlowNode>>(new Map());

  const views = [...props.views.values()];
  const { slots, columns } = treeLayout(props.views);
  const positions = new Map<NodeId, XYPosition>(
    views.map((v) => {
      const { column, row } = slots.get(v.id)!;
      return [v.id, flowState.get(String(v.id))?.position ?? { x: CARD_X + column * CARD_COLUMN_STEP, y: CARD_TOP + row * CARD_STEP }];
    }),
  );
  const lowest = Math.max(0, ...[...positions.values()].map((p) => p.y));
  const navigate = (from: NodeId) => (dir: Direction) => {
    const to = neighbor(positions, from, dir);
    if (to !== null) setSelected(to);
  };
  const keep = <N extends FlowNode>(node: N): N => ({ ...node, measured: flowState.get(node.id)?.measured });

  const nodes: FlowNode[] = [
    keep<GroupNode>({
      id: MAIN_GROUP,
      type: 'worktree',
      position: GROUP_POSITION,
      style: { width: CARD_X + columns * CARD_COLUMN_STEP, height: Math.max(GROUP_MIN_HEIGHT, lowest + CARD_STEP) },
      draggable: false,
      data: { branch: MAIN_GROUP, empty: views.length === 0, creating, onOpen: () => setCreating(true) },
    }),
    ...views.map((view) =>
      keep<CardNode>({
        id: String(view.id),
        type: 'card',
        parentId: MAIN_GROUP,
        extent: 'parent',
        dragHandle: '.card-head',
        position: positions.get(view.id)!,
        data: {
          view,
          parentName: parentName(props.views, view),
          selected: selected === view.id,
          expanded: expanded === view.id,
          onSelect: () => setSelected(view.id),
          onNavigate: navigate(view.id),
          onExpand: () => {
            setSelected(view.id);
            setExpanded(view.id);
          },
        },
      }),
    ),
  ];

  const edges: Edge[] = views.flatMap((view) => {
    if (view.parent === null) return [];
    const messages = props.pulses.get(view.id);
    return [
      {
        id: `edge-${view.id}`,
        source: String(view.parent),
        target: String(view.id),
        className: messages ? 'live' : undefined,
        label: messages ? `메시지 ${messages}` : undefined,
        labelBgPadding: [8, 2],
        labelBgBorderRadius: 10,
        focusable: false,
      },
    ];
  });

  const onNodesChange = (changes: NodeChange<FlowNode>[]) => {
    setFlowState(new Map(applyNodeChanges(changes, nodes).map((n) => [n.id, n])));
  };

  const zoomedView = expanded === null ? undefined : props.views.get(expanded);

  return (
    <>
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        onNodesChange={onNodesChange}
        onPaneClick={() => {
          setSelected(null);
          setCreating(false);
        }}
        onlyRenderVisibleElements
        elementsSelectable={false}
        nodesConnectable={false}
        nodesFocusable={false}
        deleteKeyCode={null}
        zoomOnDoubleClick={false}
        minZoom={0.4}
        maxZoom={1}
      >
        {creating && (
          // 묶음 노드 안에 두면 자식 카드가 팝오버를 덮는다. 뷰포트 층에 그려 카드 위에 둔다.
          <ViewportPortal>
            <div style={{ position: 'absolute', transform: `translate(${GROUP_POSITION.x + POPOVER_OFFSET.x}px, ${GROUP_POSITION.y + POPOVER_OFFSET.y}px)` }}>
              <NewNodePopover
                branch={MAIN_GROUP}
                onCreated={(id) => {
                  setCreating(false);
                  setSelected(id);
                }}
                onCancel={() => setCreating(false)}
              />
            </div>
          </ViewportPortal>
        )}
      </ReactFlow>
      {zoomedView && (
        <div className="zoomed">
          <NodeCard
            view={zoomedView}
            parentName={parentName(props.views, zoomedView)}
            selected={selected === zoomedView.id}
            onSelect={() => setSelected(zoomedView.id)}
            onNavigate={() => {}}
            headAction={
              <button className="btn ghost sm" onClick={() => setExpanded(null)}>
                축소
              </button>
            }
          >
            <Terminal id={zoomedView.id} focused={selected === zoomedView.id} onFocus={() => setSelected(zoomedView.id)} onNavigate={() => {}} />
          </NodeCard>
        </div>
      )}
    </>
  );
}
