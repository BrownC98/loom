import { useState } from 'react';
import { nodePty, removeNode } from './api';
import { BookmarkButton } from './BookmarkButton';
import type { DefaultCardView } from './gen/DefaultCardView';
import type { NodeView } from './gen/NodeView';
import { useRename } from './NameEdit';
import { DockSummary } from './NodeCard';
import { NodeMenu } from './NodeMenu';
import { SizingTerminal, type Direction } from './Terminal';

export function TerminalDock(props: { view: NodeView; defaultView: DefaultCardView; agentLabel: string; focusAsk: number; onNavigate: (dir: Direction) => void }) {
  const { view } = props;
  const rename = useRename();
  const [refusal, setRefusal] = useState<{ node: number; text: string } | null>(null);
  const onRefusal = (text: string) => setRefusal({ node: view.id, text });
  return (
    <>
      <div className="dock-head">
        <DockSummary view={view} agentLabel={props.agentLabel} rename={rename} actions={
            <>
              <BookmarkButton view={view} />
              <NodeMenu
                view={view}
                defaultView={props.defaultView}
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
      <div className="dock-term" translate="no">
        <SizingTerminal key={view.id} src={nodePty(view.id)} node={view.id} focused focusAsk={props.focusAsk} onFocus={() => {}} onNavigate={props.onNavigate} />
      </div>
    </>
  );
}
