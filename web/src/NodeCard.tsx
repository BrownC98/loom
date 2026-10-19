import type { ReactNode } from 'react';
import type { NodeState } from './gen/NodeState';
import type { NodeView } from './gen/NodeView';
import type { Profile } from './gen/Profile';
import { navDirection, type Direction } from './Terminal';

export const PROFILE_LABEL: Record<Profile, string> = {
  claude: 'Claude Code',
  codex: 'Codex',
  command: '직접 입력',
};

function stateLabel(state: NodeState): string {
  switch (state.kind) {
    case 'running':
      return 'running';
    case 'exited':
      return `exited(${state.code})`;
  }
}

function footText(state: NodeState, selected: boolean): string {
  if (state.kind === 'exited') return `프로세스가 코드 ${state.code}로 끝났다`;
  return selected ? '입력 중' : '클릭하면 이 카드가 입력을 받는다';
}

/** 노드 카드 틀. 터미널 자리(`children`)와 머리 오른쪽 버튼(`headAction`)은 부르는 쪽이 채운다. */
export function NodeCard(props: {
  view: NodeView;
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
        <span className="profile">{PROFILE_LABEL[view.profile]}</span>
        <span className="spacer" />
        <span className={`state ${view.state.kind}`} aria-live="polite" translate="no">
          {stateLabel(view.state)}
        </span>
        {props.headAction}
      </div>
      <div className="term nodrag nowheel" translate="no">
        {props.children}
      </div>
      <div className="card-foot">{footText(view.state, selected)}</div>
    </article>
  );
}
