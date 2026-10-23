import { useState } from 'react';
import { nodePty, removeNode } from './api';
import { BookmarkButton } from './BookmarkButton';
import type { DefaultCardView } from './gen/DefaultCardView';
import type { NodeView } from './gen/NodeView';
import { useRename } from './NameEdit';
import { NodeSummary } from './NodeCard';
import { NodeMenu } from './NodeMenu';
import { SizingTerminal, type Direction } from './Terminal';

/** 심플 보기에서 고른 노드의 터미널을 여는 오른쪽 칸. 머리는 심플 카드와 같은 노드 정보다. 이벤트·커밋 그래프와 같은 자리다. */
export function TerminalDock(props: { view: NodeView; defaultView: DefaultCardView; agentLabel: string; resumable: boolean; focusAsk: number; onNavigate: (dir: Direction) => void }) {
  const { view } = props;
  // 오른쪽 칸 머리는 두 번 누르기로만 연다. 칸에 F2를 받는 포커스 자리가 없다.
  const rename = useRename();
  // 노드 메뉴가 거절당한 이유. 다음 메뉴 동작이나 다른 노드로 바꾸면 지운다.
  const [refusal, setRefusal] = useState<{ node: number; text: string } | null>(null);
  const onRefusal = (text: string) => setRefusal({ node: view.id, text });
  return (
    <>
      <div className="dock-head">
        <NodeSummary view={view} agentLabel={props.agentLabel} rename={rename} actions={
            <>
              <BookmarkButton view={view} />
              <NodeMenu
                view={view}
                defaultView={props.defaultView}
                resumable={props.resumable}
                onRemove={() => {
                  setRefusal(null);
                  removeNode(view.id).catch((err: Error) => onRefusal(err.message));
                }}
                onRefusal={onRefusal}
              />
            </>
          }
        />
      </div>
      {refusal?.node === view.id && (
        <p className="dock-refusal" role="alert">
          {refusal.text}
        </p>
      )}
      {/* 노드를 바꾸면 새 터미널을 연다. 앞 노드의 연결은 닫는다. */}
      <div className="dock-term" translate="no">
        <SizingTerminal key={view.id} src={nodePty(view.id)} node={view.id} focused focusAsk={props.focusAsk} onFocus={() => {}} onNavigate={props.onNavigate} />
      </div>
    </>
  );
}
