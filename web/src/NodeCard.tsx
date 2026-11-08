import { Mail } from 'lucide-react';
import { useRef, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { AgentMark } from './AgentMark';
import { steerQueued } from './api';
import type { NodeState } from './gen/NodeState';
import type { NodeView } from './gen/NodeView';
import type { ProcessState } from './gen/ProcessState';
import i18next from './i18n';
import { NameEdit, useRename, type Rename } from './NameEdit';
import { asking, chipClass, NoticeChips, stopped } from './notice';
import { navDirection, type Direction } from './keys';

function processClass(process: ProcessState): string {
  switch (process.kind) {
    case 'working':
      return 'running';
    case 'stopped':
      return 'stopped';
    case 'blocked':
      return processClass(process.before);
    case 'idle':
    case 'asleep':
      return '';
  }
}

export function stateClass(view: NodeView): string {
  if (asking(view).length > 0) return 'waiting';
  switch (view.state.kind) {
    case 'setupFailed':
      return 'setupFailed';
    case 'done':
      return 'done';
    case 'running':
      return processClass(view.process);
  }
}

export const stateText = (view: NodeView) => (asking(view).length > 0 ? i18next.t('wait.label') : stateTitle(view));

export const stateTitle = (view: NodeView) => `${stateLabel(view.state)} · ${processLabel(view.process)}`;

export const viewLabel = (view: NodeView) => (view.state.kind === 'setupFailed' ? stateLabel(view.state) : processLabel(view.process));

export function processLabel(process: ProcessState): string {
  const t = i18next.t;
  switch (process.kind) {
    case 'working':
      return t('node.process.working');
    case 'idle':
      return t('node.process.idle');
    case 'asleep':
      return t('node.process.asleep');
    case 'blocked':
      return t('node.process.blocked');
    case 'stopped':
      return process.code === null ? t('node.process.stopped') : t('node.process.stoppedCode', { code: process.code });
  }
}

export function stateLabel(state: NodeState): string {
  const t = i18next.t;
  switch (state.kind) {
    case 'running':
      return t('node.state.running');
    case 'done':
      return t('node.state.done');
    case 'setupFailed':
      return t('node.state.setupFailed');
  }
}

function footText(view: NodeView, parentName: string | null): string | null {
  const t = i18next.t;
  const state = view.state;
  const s = stopped(view);
  if (s) {
    if (s.code === null) return t('node.foot.daemonRestarted');
    if (parentName !== null) return t('node.foot.stoppedSent', { code: s.code, parent: parentName });
    return t('node.foot.stopped', { code: s.code });
  }
  switch (state.kind) {
    case 'setupFailed':
      return t('node.foot.setupFailed', { code: state.code });
    case 'running':
    case 'done':
      return null;
  }
}

export const modelText = (view: NodeView) => [view.model, view.effort].filter(Boolean).join(' · ');

export function AgentLine(props: { view: NodeView; label: string }) {
  return (
    <>
      <AgentMark agent={props.view.agent} label={props.label} />
      <span className="sub" translate="no">
        {modelText(props.view) || props.label}
      </span>
    </>
  );
}

function ModelText(props: { view: NodeView }) {
  const text = modelText(props.view);
  return text ? (
    <span className="model" translate="no">
      {text}
    </span>
  ) : null;
}

export function Unread(props: { n: number; children: ReactNode }) {
  return (
    <span className="unread" aria-label={i18next.t('node.unread', { n: props.n })}>
      <Mail />
      {props.children}
    </span>
  );
}

function Badges(props: { view: NodeView }) {
  const { t } = useTranslation();
  const { view } = props;
  return (
    <>
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
      {chipClass(view) && (
        <span className={`state ${chipClass(view)}`} title={stateTitle(view)} aria-live="polite" translate="no">
          {viewLabel(view)}
        </span>
      )}
    </>
  );
}

type SummaryProps = { view: NodeView; agentLabel: string; actions: ReactNode; rename: Rename };

function SummaryRow(props: SummaryProps & { className: string; afterName?: ReactNode }) {
  const { view } = props;
  return (
    <div className={props.className}>
      <AgentMark agent={view.agent} label={props.agentLabel} />
      <NameEdit id={view.id} name={view.name} rename={props.rename} />
      {props.afterName}
      <span className="spacer" />
      <Badges view={view} />
      {props.actions}
    </div>
  );
}

export function NodeSummary(props: SummaryProps) {
  const { view } = props;
  return (
    <div className="summary">
      <SummaryRow {...props} className="summary-row" />
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

export function DockSummary(props: SummaryProps) {
  return (
    <div className="summary">
      <SummaryRow {...props} className="summary-row one-row" afterName={<ModelText view={props.view} />} />
    </div>
  );
}

type ShellProps = {
  view: NodeView;
  selected: boolean;
  onSelect: () => void;
  onNavigate: (dir: Direction) => void;
};

export function isControl(target: EventTarget): boolean {
  return target instanceof Element && target.closest('button, [role="menu"]') !== null;
}

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
      onMouseDown={(e) => {
        if (!isControl(e.target)) props.onSelect();
      }}
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

export function SimpleCard(props: ShellProps & { agentLabel: string; actions: ReactNode }) {
  return (
    <CardShell className="card simple" {...props}>
      {(rename) => <NodeSummary view={props.view} agentLabel={props.agentLabel} actions={props.actions} rename={rename} />}
    </CardShell>
  );
}

export function NodeCard(
  props: ShellProps & {
    agentLabel: string;
    parentName: string | null;
    onDoubleClickHead?: () => void;
    headAction: ReactNode;
    children: ReactNode;
  },
) {
  const { view } = props;
  const foot = footText(view, props.parentName);
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
