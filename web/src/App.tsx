import { useEffect, useRef, useState } from 'react';
import { getRepo, watchGraph } from './api';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { RepoView } from './gen/RepoView';
import { Graph } from './Graph';

/** 메시지가 오간 간선을 강조하는 시간. 마지막 메시지부터 잰다. */
const PULSE_MS = 3000;

export function App() {
  const [repo, setRepo] = useState<RepoView | null>(null);
  const [nodes, setNodes] = useState<ReadonlyMap<NodeId, NodeView>>(new Map());
  // 자식 id → 강조하는 동안 그 자식의 간선으로 오간 메시지 수.
  const [pulses, setPulses] = useState<ReadonlyMap<NodeId, number>>(new Map());
  const pulseTimers = useRef(new Map<NodeId, number>());

  useEffect(() => {
    getRepo().then(setRepo);
    const pulse = (child: NodeId) => {
      setPulses((prev) => new Map(prev).set(child, (prev.get(child) ?? 0) + 1));
      clearTimeout(pulseTimers.current.get(child));
      const timer = window.setTimeout(() => {
        setPulses((prev) => {
          const next = new Map(prev);
          next.delete(child);
          return next;
        });
      }, PULSE_MS);
      pulseTimers.current.set(child, timer);
    };
    return watchGraph((event) => {
      if (event.kind === 'message') pulse(event.child);
      else setNodes((prev) => new Map(prev).set(event.id, event));
    });
  }, []);

  return (
    <div className="app">
      <nav className="side" aria-label="저장소">
        <div className="brand">loom</div>
        <h2>저장소</h2>
        {repo && (
          <ul className="repos">
            <li>
              <button className="repo" aria-current="page">
                <b>{repo.name}</b>
                <span translate="no">{repo.path}</span>
              </button>
            </li>
          </ul>
        )}
      </nav>
      <main className="canvas" aria-label={`${repo?.name ?? ''} 그래프`}>
        <div className="toolbar">
          <h1>{repo?.name}</h1>
          <span className="path" translate="no">
            {repo?.path}
          </span>
        </div>
        <Graph views={nodes} pulses={pulses} />
      </main>
    </div>
  );
}
