import { useEffect, useLayoutEffect, useRef, useState, type PointerEvent as ReactPointerEvent, type RefObject } from 'react';
import type { FloatPlace } from './gen/FloatPlace';
import type { FloatSize } from './gen/FloatSize';

export const FLOAT_MARGIN = 12;
const DRAG_SLOP = 4;
const NO_DRAG = 'input, textarea, a, label, .float-reset, .pill-close';

export const PILL_HOME: FloatPlace = { x: 0.5, y: 0 };
export const COMPOSER_HOME: FloatPlace = { x: 0.5, y: 1 };

export const FLOAT_SIZES = ['small', 'normal', 'large', 'huge'] as const satisfies readonly FloatSize[];
const ZOOM: Record<FloatSize, number> = { small: 0.85, normal: 1, large: 1.15, huge: 1.3 };
export const floatZoom = (size: FloatSize) => ZOOM[size];

export type Size = { w: number; h: number };
export type At = { left: number; top: number };
export type Rect = At & Size;

const unit = (v: number) => Math.min(1, Math.max(0, v));

const free = (pane: Size, box: Size): Size => ({ w: Math.max(0, pane.w - box.w - 2 * FLOAT_MARGIN), h: Math.max(0, pane.h - box.h - 2 * FLOAT_MARGIN) });

export function placeOf(place: FloatPlace, pane: Size, box: Size): At {
  const room = free(pane, box);
  return { left: FLOAT_MARGIN + unit(place.x) * room.w, top: FLOAT_MARGIN + unit(place.y) * room.h };
}

function placeFrom(left: number, top: number, pane: Size, box: Size): FloatPlace {
  const room = free(pane, box);
  return { x: room.w === 0 ? 0 : unit((left - FLOAT_MARGIN) / room.w), y: room.h === 0 ? 0 : unit((top - FLOAT_MARGIN) / room.h) };
}

export function useSize(ref: RefObject<HTMLElement | null>): Size | null {
  const [size, setSize] = useState<Size | null>(null);
  const watched = useRef<{ el: HTMLElement; watch: ResizeObserver } | null>(null);
  useLayoutEffect(() => {
    const el = ref.current;
    if (watched.current?.el === el) return;
    watched.current?.watch.disconnect();
    watched.current = null;
    if (!el) {
      setSize(null);
      return;
    }
    const read = () => {
      const rect = el.getBoundingClientRect();
      setSize((was) => (was && was.w === rect.width && was.h === rect.height ? was : { w: rect.width, h: rect.height }));
    };
    read();
    const watch = new ResizeObserver(read);
    watch.observe(el);
    watched.current = { el, watch };
  });
  useEffect(() => () => watched.current?.watch.disconnect(), []);
  return size;
}

export function useFloatDrag(float: RefObject<HTMLElement | null>, pane: Size | null, box: Size | null, onPlace: (place: FloatPlace) => void) {
  const [at, setAt] = useState<At | null>(null);
  const moved = useRef(false);
  const start = (e: ReactPointerEvent<HTMLElement>) => {
    const el = float.current;
    if (e.button !== 0 || !pane || !box || !el || !(e.target instanceof Element) || e.target.closest(NO_DRAG)) return;
    const from = { x: e.clientX, y: e.clientY, left: el.offsetLeft, top: el.offsetTop };
    const dragged = (ev: PointerEvent) => ({ left: from.left + ev.clientX - from.x, top: from.top + ev.clientY - from.y, far: Math.hypot(ev.clientX - from.x, ev.clientY - from.y) > DRAG_SLOP });
    const place = (ev: PointerEvent) => {
      const d = dragged(ev);
      return placeFrom(d.left, d.top, pane, box);
    };
    const move = (ev: PointerEvent) => {
      const d = dragged(ev);
      if (!d.far && !moved.current) return;
      moved.current = true;
      setAt(placeOf(place(ev), pane, box));
    };
    const end = (ev: PointerEvent) => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', end);
      window.removeEventListener('pointercancel', end);
      if (moved.current) {
        onPlace(place(ev));
        setAt(null);
        setTimeout(() => (moved.current = false));
      }
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', end);
    window.addEventListener('pointercancel', end);
  };
  const swallowClick = (e: { stopPropagation: () => void; preventDefault: () => void }) => {
    if (!moved.current) return;
    e.stopPropagation();
    e.preventDefault();
  };
  return { at, start, swallowClick };
}

export function popLeft(pillLeft: number, pillWidth: number, popWidth: number, paneWidth: number): number {
  const centred = pillLeft + (pillWidth - popWidth) / 2;
  const clamped = Math.max(FLOAT_MARGIN, Math.min(centred, paneWidth - FLOAT_MARGIN - popWidth));
  return clamped - pillLeft;
}

const POP_GAP = 6;

export function popSpace(top: number, box: Size, pane: Size, cover: Rect | null): { above: boolean; room: number } {
  const ceiling = cover && cover.top < top ? Math.min(top, cover.top + cover.h) : 0;
  const floor = cover && cover.top + cover.h > top + box.h ? Math.max(top + box.h, cover.top) : pane.h;
  const above = top - ceiling - FLOAT_MARGIN;
  const below = floor - (top + box.h) - FLOAT_MARGIN;
  const useAbove = above > below;
  const room = (useAbove ? above : below) - POP_GAP;
  if (room <= 0 && cover) return popSpace(top, box, pane, null);
  return { above: useAbove, room: Math.max(0, room) };
}
