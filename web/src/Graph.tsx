import { useState } from 'react';
import { applyNodeChanges, ReactFlow, ViewportPortal, type Node, type NodeChange, type NodeProps, type XYPosition } from '@xyflow/react';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import { NewNodePopover } from './NewNodePopover';
import { NodeCard } from './NodeCard';
import { Terminal, type Direction } from './Terminal';

// 배치 값은 승인한 목업(docs/mockups/main.html)의 좌표에서 가져왔다.
const GROUP_POSITION: XYPosition = { x: 16, y: 64 };
const GROUP_WIDTH = 440;
const GROUP_MIN_HEIGHT = 672;
const CARD_X = 20;
const CARD_TOP = 60;
/** 카드 높이에 간선이 지나갈 틈을 더한 세로 간격. */
const CARD_STEP = 328;
/** 묶음 왼쪽 위에서 잰 새 노드 팝오버 위치. */
const POPOVER_OFFSET: XYPosition = { x: 184, y: 40 };

/** 원래 체크아웃 묶음. 워크트리 묶음은 5단위에서 더한다. */
const MAIN_GROUP = 'main';

type GroupData = { branch: string; empty: boolean; creating: boolean; onOpen: () => void };
type CardData = { view: NodeView; selected: boolean; expanded: boolean; onSelect: () => void; onNavigate: (dir: Direction) => void; onExpand: () => void };
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
    <NodeCard view={data.view} selected={data.selected} onSelect={data.onSelect} onNavigate={data.onNavigate} onDoubleClickHead={data.onExpand}>
      {/* 확대한 동안에는 확대 창이 터미널을 연다. PTY 크기를 한쪽만 정하게 한다. */}
      {!data.expanded && <Terminal id={data.view.id} focused={data.selected} onFocus={data.onSelect} onNavigate={data.onNavigate} />}
    </NodeCard>
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

export function Graph(props: { views: NodeView[] }) {
  const [selected, setSelected] = useState<NodeId | null>(null);
  const [expanded, setExpanded] = useState<NodeId | null>(null);
  const [creating, setCreating] = useState(false);
  // React Flow가 잰 크기와 끈 위치. 노드를 새 객체로 넘겨도 이 값을 이어받아야 React Flow가 노드를 다시 숨기고 재지 않는다.
  const [flowState, setFlowState] = useState<ReadonlyMap<string, FlowNode>>(new Map());

  const positions = new Map<NodeId, XYPosition>(
    props.views.map((v, i) => [v.id, flowState.get(String(v.id))?.position ?? { x: CARD_X, y: CARD_TOP + i * CARD_STEP }]),
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
      style: { width: GROUP_WIDTH, height: Math.max(GROUP_MIN_HEIGHT, lowest + CARD_STEP) },
      draggable: false,
      data: { branch: MAIN_GROUP, empty: props.views.length === 0, creating, onOpen: () => setCreating(true) },
    }),
    ...props.views.map((view) =>
      keep<CardNode>({
        id: String(view.id),
        type: 'card',
        parentId: MAIN_GROUP,
        extent: 'parent',
        dragHandle: '.card-head',
        position: positions.get(view.id)!,
        data: {
          view,
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

  const onNodesChange = (changes: NodeChange<FlowNode>[]) => {
    setFlowState(new Map(applyNodeChanges(changes, nodes).map((n) => [n.id, n])));
  };

  const zoomedView = props.views.find((v) => v.id === expanded);

  return (
    <>
      <ReactFlow
        nodes={nodes}
        nodeTypes={nodeTypes}
        onNodesChange={onNodesChange}
        onPaneClick={() => {
          setSelected(null);
          setCreating(false);
        }}
        onlyRenderVisibleElements
        elementsSelectable={false}
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
