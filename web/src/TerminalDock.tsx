import { useTranslation } from 'react-i18next';
import { WebglAddon } from '@xterm/addon-webgl';
import type { Terminal as XTerm } from '@xterm/xterm';
import { nodePty } from './api';
import type { NodeView } from './gen/NodeView';
import { NodeSummary } from './NodeCard';
import { Terminal, type Direction } from './Terminal';

/**
 * 오른쪽 칸 터미널은 WebGL로 그린다. 블록 글자 사이 틈이 없다(docs/design.md 「설정」). 칸은 하나라 WebGL 컨텍스트도 하나다.
 * 컨텍스트를 잃거나 WebGL을 쓸 수 없으면 DOM 렌더러로 그린다. 애드온은 터미널을 닫을 때 함께 닫힌다.
 */
function webglRenderer(xterm: XTerm) {
  const webgl = new WebglAddon();
  webgl.onContextLoss(() => webgl.dispose());
  try {
    xterm.loadAddon(webgl);
  } catch {
    webgl.dispose();
  }
}

/** 심플 보기에서 고른 노드의 터미널을 여는 오른쪽 칸. 머리는 심플 카드와 같은 노드 정보다. 이벤트·커밋 그래프와 같은 자리다. */
export function TerminalDock(props: { view: NodeView; agentLabel: string; onNavigate: (dir: Direction) => void; onClose: () => void }) {
  const { t } = useTranslation();
  const { view } = props;
  return (
    <aside className="dock" id="dock" aria-label={t('node.label', { name: view.name })}>
      <div className="dock-head">
        <NodeSummary
          view={view}
          agentLabel={props.agentLabel}
          actions={
            <button className="btn ghost sm icon" aria-label={t('dock.closeTerminal')} title={t('dock.closeTerminal')} onClick={props.onClose}>
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <path d="M6 6l12 12M18 6L6 18" />
              </svg>
            </button>
          }
        />
      </div>
      {/* 노드를 바꾸면 새 터미널을 연다. 앞 노드의 연결은 닫는다. */}
      <div className="dock-term" translate="no">
        <Terminal key={view.id} src={nodePty(view.id)} focused onFocus={() => {}} onNavigate={props.onNavigate} loadRenderer={webglRenderer} />
      </div>
    </aside>
  );
}
