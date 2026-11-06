import type { NodeView } from './gen/NodeView';
import type { ScreenWait } from './gen/ScreenWait';

export const TYPE_PAUSE_MS = 250;

type WaitSearch = { words: string; id: number | null };

export function waitSearch(query: string): WaitSearch | null {
  const words = query.trim().toLowerCase();
  if (!words) return null;
  const digits = words.startsWith('q') ? words.slice(1) : words;
  return { words, id: /^\d+$/.test(digits) ? Number(digits) : null };
}

const findsOpen = (s: WaitSearch, node: string, wait: ScreenWait) =>
  (wait.kind === 'question' && s.id === wait.id) || [node, wait.text, wait.instruction].some((text) => text?.toLowerCase().includes(s.words));

export function openMatches(waiting: NodeView[], query: string) {
  const s = waitSearch(query);
  if (!s) return waiting;
  return waiting.map((v) => ({ ...v, waits: v.waits.filter((w) => findsOpen(s, v.name, w)) })).filter((v) => v.waits.length > 0);
}
