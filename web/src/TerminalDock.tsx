import { WebglAddon } from '@xterm/addon-webgl';
import type { Terminal as XTerm } from '@xterm/xterm';
import { nodePty } from './api';
import { BookmarkButton } from './BookmarkButton';
import type { NodeView } from './gen/NodeView';
import { useRename } from './NameEdit';
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
        <Terminal key={view.id} src={nodePty(view.id)} focused focusAsk={props.focusAsk} onFocus={() => {}} onNavigate={props.onNavigate} loadRenderer={webglRenderer} />
      </div>
    </>
  );
}
