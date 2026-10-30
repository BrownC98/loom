import { useSyncExternalStore } from 'react';

const drafts = new Map<string, string>();
const heard = new Set<() => void>();

const listen = (heed: () => void) => {
  heard.add(heed);
  return () => void heard.delete(heed);
};

export const readDraft = (key: string) => drafts.get(key) ?? '';

export function editDraft(key: string, change: (now: string) => string) {
  const next = change(readDraft(key));
  if (next) drafts.set(key, next);
  else drafts.delete(key);
  heard.forEach((heed) => heed());
}

export function useDraft(key: string): [string, (text: string) => void] {
  const text = useSyncExternalStore(listen, () => readDraft(key));
  return [text, (next) => editDraft(key, () => next)];
}
