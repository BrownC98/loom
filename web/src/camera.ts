import { useRef } from 'react';
import { useReactFlow, useStoreApi } from '@xyflow/react';
import type { NodeId } from './gen/NodeId';
import type { Spot } from './gen/Spot';

export const MOVE_MS = 300;

type ChipPaths = { chips: readonly { path: string }[] };

function useShowWhenDrawn<T>(idOf: (target: T) => string | undefined): (targets: T[]) => void {
  const flow = useReactFlow();
  const store = useStoreApi();
  const waiting = useRef<(() => void) | null>(null);
  const drawn = (id: string | undefined) => {
    const node = id === undefined ? undefined : store.getState().nodeLookup.get(id);
    return Boolean(node?.measured.width && node.measured.height);
  };
  return (targets) => {
    waiting.current?.();
    waiting.current = null;
    const ready = () => {
      const ids = targets.map(idOf);
      return ids.every(drawn) ? (ids as string[]) : null;
    };
    const fit = (ids: string[]) => void flow.fitView({ nodes: ids.map((id) => ({ id })), duration: MOVE_MS, maxZoom: 1 });
    const now = ready();
    if (now) return fit(now);
    const stop = store.subscribe(() => {
      const ids = ready();
      if (!ids) return;
      stop();
      waiting.current = null;
      fit(ids);
    });
    waiting.current = stop;
  };
}

export function useShowNodes(): (ids: NodeId[]) => void {
  return useShowWhenDrawn((id: NodeId) => String(id));
}

export function useShowGroups(): (paths: string[]) => void {
  const flow = useReactFlow();
  return useShowWhenDrawn((path: string) => (flow.getNode(path) ? path : flow.getNodes().find((n) => n.type === 'worktree' && (n.data as ChipPaths).chips.some((c) => c.path === path))?.id));
}

export function useShowAll(): () => void {
  const flow = useReactFlow();
  return () => flow.fitView({ duration: MOVE_MS, maxZoom: 1 });
}

export function flowId(spot: Spot): string {
  return spot.kind === 'repo' ? `repo:${spot.repo}` : spot.kind === 'worktree' ? spot.path : String(spot.node);
}
