import { useTranslation } from 'react-i18next';
import { AgentMark } from './AgentMark';
import { nodePty } from './api';
import type { NodeView } from './gen/NodeView';
import { Terminal, type Direction } from './Terminal';

/** 카드를 머리만 보일 때 고른 노드의 터미널을 여는 오른쪽 칸. 이벤트·커밋 그래프와 같은 자리다. */
export function TerminalDock(props: { view: NodeView; agentLabel: string; onNavigate: (dir: Direction) => void; onClose: () => void }) {
  const { t } = useTranslation();
  const { view } = props;
  return (
    <aside className="dock" id="dock" aria-label={t('node.label', { name: view.name })}>
      <div className="dock-head">
        <h2>{view.name}</h2>
        <AgentMark agent={view.agent} label={props.agentLabel} />
        <span className="spacer" />
        <button className="btn ghost sm" aria-label={t('dock.closeTerminal')} onClick={props.onClose}>
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M6 6l12 12M18 6L6 18" />
          </svg>
        </button>
      </div>
      {/* 노드를 바꾸면 새 터미널을 연다. 앞 노드의 연결은 닫는다. */}
      <div className="dock-term" translate="no">
        <Terminal key={view.id} src={nodePty(view.id)} focused onFocus={() => {}} onNavigate={props.onNavigate} />
      </div>
    </aside>
  );
}
