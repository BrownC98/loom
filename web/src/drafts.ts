import { useSyncExternalStore } from 'react';

const drafts = new Map<string, string>();
const heard = new Set<() => void>();

const listen = (heed: () => void) => {
  heard.add(heed);
  return () => void heard.delete(heed);
};

export function useDraft(key: string): [string, (text: string) => void] {
  const text = useSyncExternalStore(listen, () => drafts.get(key) ?? '');
  const write = (next: string) => {
    if (next) drafts.set(key, next);
    else drafts.delete(key);
    heard.forEach((heed) => heed());
  };
  return [text, write];
}
