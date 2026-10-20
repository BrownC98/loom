import { Mail } from 'lucide-react';
import { useRef, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { AgentMark } from './AgentMark';
import type { NodeState } from './gen/NodeState';
import type { NodeView } from './gen/NodeView';
import i18next from './i18n';
import { NameEdit, useRename, type Rename } from './NameEdit';
import { navDirection, type Direction } from './Terminal';

export function stateLabel(state: NodeState): string {
  const t = i18next.t;
  switch (state.kind) {
    case 'running':
      return t('node.state.running');
    case 'done':
      return t('node.state.done');
    case 'exited':
      return t('node.state.exited', { code: state.code ?? '?' });
  }
}

/** 도는 노드는 발 줄이 없다. 터미널이 상태를 보인다. */
function footText(state: NodeState, parentName: string | null, held: boolean): string | null {
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
      return null;
  }
}

/** 모델 · 추론 강도. 모르면 그리지 않는다. */
function ModelText(props: { view: NodeView }) {
  const text = [props.view.model, props.view.effort].filter(Boolean).join(' · ');
  return text ? (
    <span className="model" translate="no">
      {text}
    </span>
  ) : null;
}

/** 읽지 않은 메시지 표시(목업 docs/mockups/unread-badge.html C안). 봉투 아이콘 뒤에 `children`을 둔다. 카드는 「안 읽음 N」, 트리 줄은 수만 둔다. */
export function Unread(props: { n: number; children: ReactNode }) {
  return (
    <span className="unread" aria-label={i18next.t('node.unread', { n: props.n })}>
      <Mail />
      {props.children}
    </span>
  );
}

/** 읽지 않은 수, 보관 수, 상태. */
function Badges(props: { view: NodeView }) {
  const { t } = useTranslation();
  const { view } = props;
  return (
    <>
      {view.unread > 0 && <Unread n={view.unread}>{t('node.unreadShort', { n: view.unread })}</Unread>}
      {view.held > 0 && (
        <span className="held" title={t('node.heldTitle', { n: view.held })}>
          {t('node.held', { n: view.held })}
        </span>
      )}
      <span className={`state ${view.state.kind}`} aria-live="polite" translate="no">
        {stateLabel(view.state)}
      </span>
    </>
  );
}

/**
 * 노드 정보 전체: 로고·이름·버튼(`actions`), 지금 하는 일, 모델 · 추론 강도, 상태와 수. 심플 카드와 오른쪽 칸 터미널의 머리가 함께 쓴다.
 * 목업 docs/mockups/card-view.html 1번.
 */
export function NodeSummary(props: { view: NodeView; agentLabel: string; actions: ReactNode; rename: Rename }) {
  const { view } = props;
  return (
    <div className="summary">
      <div className="summary-row">
        <AgentMark agent={view.agent} label={props.agentLabel} />
        <NameEdit id={view.id} name={view.name} rename={props.rename} />
        <span className="spacer" />
        {props.actions}
      </div>
      {view.topic && (
        <p className="topic" title={view.topic}>
          {view.topic}
        </p>
      )}
      {(view.model || view.effort) && (
        <div className="summary-model">
          <ModelText view={view} />
        </div>
      )}
      <div className="summary-row">
        <Badges view={view} />
      </div>
    </div>
  );
}

type ShellProps = {
  view: NodeView;
  selected: boolean;
  onSelect: () => void;
  onNavigate: (dir: Direction) => void;
};

/** 카드 틀: 고르기, 키보드 이동, F2 이름 바꾸기. 이름 편집 상태를 `children`에 넘긴다. */
function CardShell(props: ShellProps & { className: string; children: (rename: Rename) => ReactNode }) {
  const { t } = useTranslation();
  const home = useRef<HTMLElement>(null);
  const rename = useRename(home);
  return (
    <article
      ref={home}
      className={props.className}
      tabIndex={0}
      aria-label={t('node.label', { name: props.view.name })}
      data-selected={props.selected || undefined}
      onMouseDown={props.onSelect}
      onKeyDown={(e) => {
        if (e.target !== e.currentTarget) return;
        const dir = navDirection(e);
        if (dir) {
          e.preventDefault();
          props.onNavigate(dir);
        } else if (e.key === 'Enter') {
          props.onSelect();
        } else if (e.key === 'F2') {
          e.preventDefault();
          rename.open();
        }
      }}
    >
      {props.children(rename)}
    </article>
  );
}

/** 심플 카드. 터미널이 없다. 고르면 부르는 쪽이 오른쪽 칸에 터미널을 연다. */
export function SimpleCard(props: ShellProps & { agentLabel: string; actions: ReactNode }) {
  return (
    <CardShell className="card simple" {...props}>
      {(rename) => <NodeSummary view={props.view} agentLabel={props.agentLabel} actions={props.actions} rename={rename} />}
    </CardShell>
  );
}

/** 표준 카드: 한 줄 머리, 터미널(`children`), 끝난 노드의 발 줄. 머리 오른쪽 버튼(`headAction`)은 부르는 쪽이 채운다. */
export function NodeCard(
  props: ShellProps & {
    /** 에이전트 이름(카탈로그 label)이나 「터미널」. */
    agentLabel: string;
    /** 부모가 없으면 null. 끝난 노드의 발 줄에 쓴다. 부모가 없는 동안 보낸 메시지는 데몬이 보관한다(`view.held`). */
    parentName: string | null;
    onDoubleClickHead?: () => void;
    headAction: ReactNode;
    children: ReactNode;
  },
) {
  const { view } = props;
  const foot = footText(view.state, props.parentName, view.held > 0);
  return (
    <CardShell className="card" {...props}>
      {(rename) => (
        <>
          <div className="card-head" onDoubleClick={props.onDoubleClickHead}>
            <NameEdit id={view.id} name={view.name} rename={rename} />
            <AgentMark agent={view.agent} label={props.agentLabel} />
            <ModelText view={view} />
            <span className="spacer" />
            <Badges view={view} />
            {props.headAction}
          </div>
          <div className="term nodrag nowheel" translate="no">
            {props.children}
          </div>
          {foot !== null && <div className="card-foot">{foot}</div>}
        </>
      )}
    </CardShell>
  );
}
