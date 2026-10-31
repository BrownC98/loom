import { useEffect, useRef, type RefObject } from 'react';

type Open = { box: RefObject<HTMLElement | null>; close: RefObject<() => void> };

const opened: Open[] = [];

document.addEventListener(
  'keydown',
  (e) => {
    if (e.key !== 'Escape') return;
    const top = opened.at(-1);
    const box = top?.box.current;
    if (!top || !box) return;
    const modal = document.querySelector('dialog:modal');
    if (modal && !modal.contains(box)) return;
    e.preventDefault();
    e.stopPropagation();
    top.close.current();
  },
  true,
);

export function useEscapeClose<T extends HTMLElement>(onClose: () => void) {
  const box = useRef<T>(null);
  useEscapeEntry(box, onClose);
  return box;
}

export function useDismiss<T extends HTMLElement>(onClose: () => void, options: { anchor?: RefObject<HTMLElement | null> } = {}) {
  const box = useRef<T>(null);
  const { anchor } = options;
  useOutsidePress(box, onClose, anchor);
  useEscapeEntry(box, () => {
    onClose();
    anchor?.current?.focus();
  });
  return box;
}

function useEscapeEntry(box: RefObject<HTMLElement | null>, onClose: () => void) {
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    const entry: Open = { box, close };
    opened.push(entry);
    return () => void opened.splice(opened.indexOf(entry), 1);
  }, []);
}

export function useOutsidePress(box: RefObject<HTMLElement | null>, onClose: () => void, anchor?: RefObject<HTMLElement | null>) {
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    const outside = (e: PointerEvent) => {
      const target = e.target as Node;
      if (box.current?.querySelector('[aria-busy="true"]')) return;
      const modal = document.querySelector('dialog:modal');
      if (modal && !modal.contains(box.current)) return;
      if (!box.current?.contains(target) && !anchor?.current?.contains(target)) close.current();
    };
    document.addEventListener('pointerdown', outside, true);
    return () => document.removeEventListener('pointerdown', outside, true);
  }, []);
}
