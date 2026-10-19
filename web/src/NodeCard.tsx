import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { NodeState } from './gen/NodeState';
import type { NodeView } from './gen/NodeView';
import i18next from './i18n';
import { navDirection, type Direction } from './Terminal';

export function stateLabel(state: NodeState): string {
  switch (state.kind) {
    case 'running':
      return 'running';
    case 'done':
      return 'done';
    case 'exited':
      return `exited(${state.code ?? '?'})`;
  }
}

function footText(state: NodeState, parentName: string | null, held: boolean, selected: boolean): string {
  const t = i18next.t;
  switch (state.kind) {
    case 'done':
      return held ? t('node.foot.doneHeld') : t('node.foot.done', { summary: state.summary });
    case 'exited':
      if (state.code === null) return t('node.foot.daemonRestarted');
      if (held) return t('node.foot.exitedHeld', { code: state.code });
      if (parentName !== null) return t('node.foot.exitedSent', { code: state.code, parent: parentName });
      return t('node.foot.exited', { code: state.code });
    case 'running':
      return selected ? t('node.foot.typing') : t('node.foot.clickToType');
  }
}

/** 노드 카드 틀. 터미널 자리(`children`)와 머리 오른쪽 버튼(`headAction`)은 부르는 쪽이 채운다. */
export function NodeCard(props: {
  view: NodeView;
  /** 에이전트 이름(카탈로그 label)이나 「직접 입력」. */
  agentLabel: string;
  /** 부모가 없으면 null. 끝난 노드의 발 줄에 쓴다. 부모가 없는 동안 보낸 메시지는 데몬이 보관한다(`view.held`). */
  parentName: string | null;
  selected: boolean;
  onSelect: () => void;
  onNavigate: (dir: Direction) => void;
  onDoubleClickHead?: () => void;
  headAction?: ReactNode;
  children: ReactNode;
}) {
  const { t } = useTranslation();
  const { view, selected } = props;
  return (
    <article
      className="card"
      tabIndex={0}
      aria-label={t('node.label', { name: view.name })}
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
          <span className="unread" aria-label={t('node.unread', { n: view.unread })}>
            {view.unread}
          </span>
        )}
        {view.held > 0 && (
          <span className="held" title={t('node.heldTitle', { n: view.held })}>
            {t('node.held', { n: view.held })}
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
      <div className="card-foot">{footText(view.state, props.parentName, view.held > 0, selected)}</div>
    </article>
  );
}
