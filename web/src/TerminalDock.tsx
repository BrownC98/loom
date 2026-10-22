import { nodePty } from './api';
import { BookmarkButton } from './BookmarkButton';
import type { NodeView } from './gen/NodeView';
import { useRename } from './NameEdit';
import { NodeSummary } from './NodeCard';
import { SizingTerminal, type Direction } from './Terminal';

/** 심플 보기에서 고른 노드의 터미널을 여는 오른쪽 칸. 머리는 심플 카드와 같은 노드 정보다. 이벤트·커밋 그래프와 같은 자리다. */
export function TerminalDock(props: { view: NodeView; agentLabel: string; focusAsk: number; onNavigate: (dir: Direction) => void }) {
  const { view } = props;
  // 오른쪽 칸 머리는 두 번 누르기로만 연다. 칸에 F2를 받는 포커스 자리가 없다.
  const rename = useRename();
  return (
    <>
      <div className="dock-head">
        <NodeSummary view={view} agentLabel={props.agentLabel} rename={rename} actions={<BookmarkButton view={view} />} />
      </div>
      {/* 노드를 바꾸면 새 터미널을 연다. 앞 노드의 연결은 닫는다. */}
      <div className="dock-term" translate="no">
        <SizingTerminal key={view.id} src={nodePty(view.id)} node={view.id} focused focusAsk={props.focusAsk} onFocus={() => {}} onNavigate={props.onNavigate} />
      </div>
    </>
  );
}
