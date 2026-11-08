import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { ScreenWait } from './gen/ScreenWait';
import type { WaitId } from './gen/WaitId';
import { bySince } from './notice';

export const PROMPT = 'prompt';

export type TargetKey = WaitId | typeof PROMPT;

export type Chosen = { node: NodeId; key: TargetKey };

export type Route = { kind: 'answer'; wait: ScreenWait } | { kind: 'continue'; wait: ScreenWait } | { kind: 'prompt' };

export type StreamWait = { view: NodeView; wait: ScreenWait };

export type Target = {
  view: NodeView;
  stream: readonly StreamWait[];
  key: TargetKey;
  card: ScreenWait | null;
  lost: boolean;
  route: Route;
};

export const byStream = (a: ScreenWait, b: ScreenWait) => bySince(a, b) || a.id - b.id;

export const streamOf = (waiting: readonly NodeView[]): StreamWait[] => waiting.flatMap((view) => view.waits.map((wait) => ({ view, wait }))).sort((a, b) => byStream(a.wait, b.wait));

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
    key,
    card,
    lost: pinned !== null && (held === null || !open(held.key)),
    route: routeOf(key === PROMPT ? null : card),
    stream: streamOf(waiting),
  };
}

export function stepTo(target: Target, step: 1 | -1): Chosen {
  const stops: Chosen[] = [...target.stream.map((s) => ({ node: s.view.id, key: s.wait.id })), { node: target.view.id, key: PROMPT }];
  const at = stops.findIndex((s) => s.node === target.view.id && s.key === target.key);
  return stops[((at < 0 ? stops.length - 1 : at) + step + stops.length) % stops.length];
}

export function followingWait(stream: readonly StreamWait[], gone: ScreenWait): Chosen | null {
  const next = stream.find((s) => byStream(s.wait, gone) > 0) ?? stream[0];
  return next ? { node: next.view.id, key: next.wait.id } : null;
}
