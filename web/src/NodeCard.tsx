import { Mail } from 'lucide-react';
import { useRef, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { AgentMark } from './AgentMark';
import { steerQueued } from './api';
import type { NodeState } from './gen/NodeState';
import type { NodeView } from './gen/NodeView';
import i18next from './i18n';
import { NameEdit, useRename, type Rename } from './NameEdit';
import { asking, NoticeChips } from './notice';
import { navDirection, type Direction } from './Terminal';

/**
 * 노드 점의 클래스. 사람 응답 대기는 실행 상태보다 앞선다(목업 docs/mockups/human-question.html ④).
 * 실행 중·완료는 색이 없다. 쉬는 노드도 「실행 중」으로 보여 헷갈렸다(2026-10-23 사람 지시 "사이드바 표시에도 적용").
 */
export function stateClass(view: NodeView): string {
  if (asking(view).length > 0) return 'waiting';
  return view.state.kind === 'exited' || view.state.kind === 'setupFailed' ? view.state.kind : '';
}

/** 점의 글: 사람 응답 대기는 실행 상태보다 앞선다. 점의 클래스(`stateClass`)와 짝이다. */
export const stateText = (view: NodeView) => (asking(view).length > 0 ? i18next.t('wait.label') : stateLabel(view.state));

export function stateLabel(state: NodeState): string {
  const t = i18next.t;
  switch (state.kind) {
    case 'running':
      return t('node.state.running');
    case 'done':
      return t('node.state.done');
    case 'exited':
      return t('node.state.exited', { code: state.code ?? '?' });
    case 'setupFailed':
      return t('node.state.setupFailed');
  }
}

/** 도는 노드와 끝난 노드는 발 줄이 없다. 완료 글은 받은편지함과 부모가 받는다(2026-10-23 사람 지시 "요약 기능 제거"). */
function footText(state: NodeState, parentName: string | null): string | null {
  const t = i18next.t;
  switch (state.kind) {
    case 'exited':
      if (state.code === null) return t('node.foot.daemonRestarted');
      if (parentName !== null) return t('node.foot.exitedSent', { code: state.code, parent: parentName });
      return t('node.foot.exited', { code: state.code });
    case 'setupFailed':
      return t('node.foot.setupFailed', { code: state.code });
    case 'running':
    case 'done':
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

/** 읽지 않은 수와 상태. */
function Badges(props: { view: NodeView }) {
  const { t } = useTranslation();
  const { view } = props;
  return (
    <>
      {/* 누르면 기다리는 메시지를 지금 넘긴다. 에이전트가 턴 끝까지 읽지 않을 때 쓴다(2026-10-22 사람 지시 "해당 칩을 클릭하면 수동으로 주입되게"). */}
      {view.unread > 0 && (
        <button
          type="button"
          className="unread nodrag"
          title={t('node.steerQueued')}
          aria-label={`${t('node.unread', { n: view.unread })}. ${t('node.steerQueued')}`}
          onClick={(e) => {
            e.stopPropagation();
            void steerQueued(view.id);
          }}
        >
          <Mail />
          {t('node.unreadShort', { n: view.unread })}
        </button>
      )}
      <NoticeChips view={view} />
      {/* 실행 중·완료는 칩이 없다. 사람이 할 일이 있는 종료·설정 실패만 보인다(노드 줄 StateTag와 같은 규칙, 2026-10-23 사람 지시: 쉬는 노드도 「실행 중」으로 보여 헷갈렸다). */}
      {(view.state.kind === 'exited' || view.state.kind === 'setupFailed') && (
        <span className={`state ${view.state.kind}`} aria-live="polite" translate="no">
          {stateLabel(view.state)}
        </span>
      )}
    </>
  );
}

/**
 * 노드 정보 전체: 로고·이름·상태와 수·버튼(`actions`), 모델 · 추론 강도, 상태 줄(마지막 턴 끝 답 글). 상태는 이름 줄 오른쪽이다(2026-10-21 사람 지시). 심플 카드와 오른쪽 칸 터미널의 머리가 함께 쓴다.
 * 목업 docs/mockups/card-view.html 1번, node-card-view.html ⑧. 상태 줄은 두 줄까지 보이고, 마우스를 올리면 전문이 보인다.
 */
export function NodeSummary(props: { view: NodeView; agentLabel: string; actions: ReactNode; rename: Rename }) {
  const { view } = props;
  return (
    <div className="summary">
      <div className="summary-row">
        <AgentMark agent={view.agent} label={props.agentLabel} />
        <NameEdit id={view.id} name={view.name} rename={props.rename} />
        <span className="spacer" />
        <Badges view={view} />
        {props.actions}
      </div>
      {(view.model || view.effort) && (
        <div className="summary-model">
          <ModelText view={view} />
        </div>
      )}
      {view.reply && (
        <p className="summary-reply" title={view.reply}>
          {view.reply}
        </p>
      )}
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
      data-waiting={asking(props.view).length > 0 || undefined}
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
    /** 부모가 없으면 null. 끝난 노드의 발 줄에 쓴다. */
    parentName: string | null;
    onDoubleClickHead?: () => void;
    headAction: ReactNode;
    children: ReactNode;
  },
) {
  const { view } = props;
  const foot = footText(view.state, props.parentName);
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
