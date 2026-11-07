import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { ScreenWait } from './gen/ScreenWait';
import type { WaitId } from './gen/WaitId';
import { bySince } from './notice';

export const PROMPT = 'prompt';

export type TargetKey = WaitId | typeof PROMPT;

export type Chosen = { node: NodeId; key: TargetKey };

export type Route = { kind: 'answer'; wait: ScreenWait } | { kind: 'continue'; wait: ScreenWait } | { kind: 'prompt' };

export type Target = {
  view: NodeView;
  waits: readonly ScreenWait[];
  key: TargetKey;
  card: ScreenWait | null;
  lost: boolean;
  route: Route;
  index: number;
  total: number;
};

function routeOf(wait: ScreenWait | null): Route {
  if (!wait || !wait.answerable) return { kind: 'prompt' };
  return { kind: wait.tone === 'done' ? 'continue' : 'answer', wait };
}

export function resolveTarget(nodes: ReadonlyMap<NodeId, NodeView>, waiting: readonly NodeView[], selected: NodeId | null, chosen: Chosen | null, pinned: Chosen | null): Target | null {
  const view = (pinned && nodes.get(pinned.node)) || (selected !== null && nodes.get(selected)) || waiting[0];
  if (!view) return null;
  const held = pinned?.node === view.id ? pinned : null;
  const waits = [...view.waits].sort(bySince);
  const open = (wanted: TargetKey) => wanted === PROMPT || waits.some((w) => w.id === wanted);
  const wanted = chosen?.node === view.id ? chosen.key : null;
  const key: TargetKey = held ? held.key : wanted !== null && open(wanted) ? wanted : (waits[0]?.id ?? PROMPT);
  const card = key === PROMPT ? (waits[0] ?? null) : (waits.find((w) => w.id === key) ?? null);
  return {
    view,
    waits,
    key,
    card,
    lost: pinned !== null && (held === null || !open(held.key)),
    route: routeOf(key === PROMPT ? null : card),
    index: card ? waits.indexOf(card) + 1 : 0,
    total: waiting.reduce((n, v) => n + v.waits.length, 0),
  };
}

export function stepKey(target: Target, step: 1 | -1): TargetKey {
  const keys: TargetKey[] = [...target.waits.map((w) => w.id), PROMPT];
  return keys[(keys.indexOf(target.key) + step + keys.length) % keys.length];
}
