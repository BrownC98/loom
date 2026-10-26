import { useRef } from 'react';
import { useReactFlow, useStoreApi } from '@xyflow/react';
import type { NodeId } from './gen/NodeId';
import type { Spot } from './gen/Spot';

export const MOVE_MS = 300;

type ChipPaths = { chips: readonly { path: string }[] };

export function useShowNodes(): (ids: NodeId[]) => void {
  const flow = useReactFlow();
  return (ids) => flow.fitView({ nodes: ids.map((id) => ({ id: String(id) })), duration: MOVE_MS, maxZoom: 1 });
}

export function useShowGroups(): (paths: string[]) => void {
  const flow = useReactFlow();
  const store = useStoreApi();
  const waiting = useRef<(() => void) | null>(null);
  const boxOf = (path: string) => (flow.getNode(path) ? path : flow.getNodes().find((n) => n.type === 'worktree' && (n.data as ChipPaths).chips.some((c) => c.path === path))?.id);
  return (paths) => {
    waiting.current?.();
    waiting.current = null;
    const targets = () => {
      const ids = paths.map(boxOf);
      return ids.every((id) => id !== undefined) ? (ids as string[]) : null;
    };
    const fit = (ids: string[]) => void flow.fitView({ nodes: ids.map((id) => ({ id })), duration: MOVE_MS, maxZoom: 1 });
    const now = targets();
    if (now) return fit(now);
    const stop = store.subscribe(() => {
      const ids = targets();
      if (!ids) return;
      stop();
      waiting.current = null;
      fit(ids);
    });
    waiting.current = stop;
  };
}

export function useShowAll(): () => void {
  const flow = useReactFlow();
  return () => flow.fitView({ duration: MOVE_MS, maxZoom: 1 });
}

export function flowId(spot: Spot): string {
  return spot.kind === 'repo' ? `repo:${spot.repo}` : spot.kind === 'worktree' ? spot.path : String(spot.node);
}
