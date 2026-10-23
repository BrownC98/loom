import { useEffect, useState, type KeyboardEvent, type ReactNode, type RefObject } from 'react';
import { createPortal } from 'react-dom';
import { useDismiss } from './popover';

const MENU_WIDTH = 180;

export function Menu(props: { anchor: RefObject<HTMLElement | null>; label: string; onClose: () => void; children: ReactNode }) {
  const box = useDismiss<HTMLDivElement>(props.onClose, { anchor: props.anchor });
  const [at] = useState(() => {
    const r = props.anchor.current?.getBoundingClientRect();
    return r ? { top: r.bottom + 4, left: r.right - MENU_WIDTH } : { top: 0, left: 0 };
  });
  const live = () => [...(box.current?.querySelectorAll<HTMLButtonElement>('button:enabled') ?? [])];
  useEffect(() => {
    if (!box.current?.contains(document.activeElement)) live()[0]?.focus();
  }, []);
  const onKey = (e: KeyboardEvent) => {
    const items = live();
    const now = items.indexOf(document.activeElement as HTMLButtonElement);
    const step = e.key === 'ArrowDown' ? 1 : e.key === 'ArrowUp' ? -1 : 0;
    if (step) {
      e.preventDefault();
      items[(now + step + items.length) % items.length]?.focus();
    }
  };
  return createPortal(
    <div ref={box} className="menu" role="menu" aria-label={props.label} style={at} onKeyDown={onKey}>
      {props.children}
    </div>,
    document.body,
  );
}
