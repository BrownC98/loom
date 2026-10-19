import { useEffect, useState } from 'react';
import { getRepo, watchNodes } from './api';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { RepoView } from './gen/RepoView';
import { Graph } from './Graph';

export function App() {
  const [repo, setRepo] = useState<RepoView | null>(null);
  const [nodes, setNodes] = useState<ReadonlyMap<NodeId, NodeView>>(new Map());

  useEffect(() => {
    getRepo().then(setRepo);
    return watchNodes((view) => setNodes((prev) => new Map(prev).set(view.id, view)));
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
        <Graph views={[...nodes.values()]} />
      </main>
    </div>
  );
}
