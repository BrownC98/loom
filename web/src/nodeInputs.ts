import { useSyncExternalStore } from 'react';
import type { NodeId } from './gen/NodeId';

export type InputState = 'open' | 'offline' | 'gone';

export type NodeInput = { state: () => InputState; paste: (text: string) => void; enter: () => void };

const inputs = new Map<NodeId, readonly NodeInput[]>();
const heard = new Set<() => void>();

const tell = () => heard.forEach((heed) => heed());
const listen = (heed: () => void) => {
  heard.add(heed);
  return () => void heard.delete(heed);
};

export function offerInput(node: NodeId, input: NodeInput): () => void {
  inputs.set(node, [...(inputs.get(node) ?? []), input]);
  tell();
  return () => {
    const rest = (inputs.get(node) ?? []).filter((i) => i !== input);
    if (rest.length > 0) inputs.set(node, rest);
    else inputs.delete(node);
    tell();
  };
}

export const useNodeInput = (node: NodeId | null): NodeInput | null => useSyncExternalStore(listen, () => (node === null ? null : (inputs.get(node)?.[0] ?? null)));
