import { type RefObject, useLayoutEffect, useRef, useState } from 'react';
import { waitSearch } from './waitSearch';

const ALL = 'inbox-find';
const NOW = 'inbox-find-now';

export type InboxFind = { total: number; at: number; step: (by: 1 | -1) => void };

function findRanges(root: HTMLElement, query: string): Range[] {
  const s = waitSearch(query);
  if (!s) return [];
  const ranges: Range[] = [];
  for (const el of root.querySelectorAll<HTMLElement>('[data-find], [data-find-id]')) {
    if (el.dataset.findId !== undefined) {
      if (s.id !== null && Number(el.dataset.findId) === s.id) {
        const range = new Range();
        range.selectNodeContents(el);
        ranges.push(range);
      }
      continue;
    }
    const walk = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    while (walk.nextNode()) {
      const node = walk.currentNode as Text;
      const text = node.data.toLowerCase();
      if (text.length !== node.length) continue;
      for (let at = text.indexOf(s.words); at >= 0; at = text.indexOf(s.words, at + s.words.length)) {
        const range = new Range();
        range.setStart(node, at);
        range.setEnd(node, at + s.words.length);
        ranges.push(range);
      }
    }
  }
  return ranges;
}

export function useInboxFind(root: RefObject<HTMLElement | null>, query: string): InboxFind {
  const [total, setTotal] = useState(0);
  const [at, setAt] = useState(0);
  const move = useRef<(by: 1 | -1) => void>(() => {});
  useLayoutEffect(() => {
    const el = root.current;
    if (!el) return;
    let found: Range[] = [];
    let now = 0;
    const paint = () => {
      CSS.highlights?.set(ALL, new Highlight(...found));
      const current = new Highlight(...found.slice(now, now + 1));
      current.priority = 1;
      CSS.highlights?.set(NOW, current);
      setTotal(found.length);
      setAt(now);
    };
    const scan = () => {
      found = findRanges(el, query);
      now = Math.min(now, Math.max(found.length - 1, 0));
      paint();
    };
    move.current = (by) => {
      if (found.length === 0) return;
      now = (now + by + found.length) % found.length;
      paint();
      const target = found[now].startContainer;
      (target instanceof Element ? target : target.parentElement)?.scrollIntoView({ block: 'nearest' });
    };
    scan();
    const watch = new MutationObserver(scan);
    watch.observe(el, { childList: true, subtree: true, characterData: true });
    return () => {
      watch.disconnect();
      CSS.highlights?.delete(ALL);
      CSS.highlights?.delete(NOW);
    };
  }, [root, query]);
  return { total, at, step: (by) => move.current(by) };
}
