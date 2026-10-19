import type { ReactNode } from 'react';
import type { NodeState } from './gen/NodeState';
import type { NodeView } from './gen/NodeView';
import { navDirection, type Direction } from './Terminal';

function stateLabel(state: NodeState): string {
  switch (state.kind) {
    case 'running':
      return 'running';
    case 'done':
      return 'done';
    case 'exited':
      return `exited(${state.code ?? '?'})`;
  }
}

function footText(state: NodeState, parentName: string | null, selected: boolean): string {
  switch (state.kind) {
    case 'done':
      return `요약: ${state.summary}`;
    case 'exited':
      if (state.code === null) return '데몬이 다시 시작해 프로세스가 없다';
      if (parentName !== null) return `부모(${parentName})에게 exited(${state.code})을 보냈다`;
      return `프로세스가 코드 ${state.code}로 끝났다`;
    case 'running':
      return selected ? '입력 중' : '클릭하면 이 카드가 입력을 받는다';
  }
}

/** 노드 카드 틀. 터미널 자리(`children`)와 머리 오른쪽 버튼(`headAction`)은 부르는 쪽이 채운다. */
export function NodeCard(props: {
  view: NodeView;
  /** 에이전트 이름(카탈로그 label)이나 「직접 입력」. */
  agentLabel: string;
  /** 부모가 없으면 null. 끝난 노드의 발 줄에 쓴다. */
  parentName: string | null;
  selected: boolean;
  onSelect: () => void;
  onNavigate: (dir: Direction) => void;
  onDoubleClickHead?: () => void;
  headAction?: ReactNode;
  children: ReactNode;
}) {
  const { view, selected } = props;
  return (
    <article
      className="card"
      tabIndex={0}
      aria-label={`노드 ${view.name}`}
      data-selected={selected || undefined}
      onMouseDown={props.onSelect}
      onKeyDown={(e) => {
        if (e.target !== e.currentTarget) return;
        const dir = navDirection(e);
        if (dir) {
          e.preventDefault();
          props.onNavigate(dir);
        } else if (e.key === 'Enter') {
          props.onSelect();
        }
      }}
    >
      <div className="card-head" onDoubleClick={props.onDoubleClickHead}>
        <span className="name">{view.name}</span>
        <span className="agent">{props.agentLabel}</span>
        <span className="spacer" />
        {view.unread > 0 && (
          <span className="unread" aria-label={`읽지 않은 메시지 ${view.unread}개`}>
            {view.unread}
          </span>
        )}
        <span className={`state ${view.state.kind}`} aria-live="polite" translate="no">
          {stateLabel(view.state)}
        </span>
        {props.headAction}
      </div>
      <div className="term nodrag nowheel" translate="no">
        {props.children}
      </div>
      <div className="card-foot">{footText(view.state, props.parentName, selected)}</div>
    </article>
  );
}
