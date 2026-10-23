import { useEffect, useState, type KeyboardEvent, type PointerEvent } from 'react';
import type { PaneWidths } from './gen/PaneWidths';

export type Pane = { key: keyof PaneWidths; cssVar: '--side-w' | '--dock-w'; initial: number; min: number; max: number; grow: 1 | -1 };

const KEY_STEP = 16;

const clamp = (pane: Pane, width: number) => Math.min(pane.max, Math.max(pane.min, Math.round(width)));

export function useViewportWidth(): number {
  const [width, setWidth] = useState(() => window.innerWidth);
  useEffect(() => {
    const onResize = () => setWidth(window.innerWidth);
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, []);
  return width;
}

export const paneWidth = (pane: Pane, saved: number | null) => clamp(pane, saved ?? pane.initial);

export function Splitter(props: { pane: Pane; width: number; onCommit: (width: number) => void; label: string }) {
  const { pane, width } = props;
  const onCommit = (next: number) => props.onCommit(clamp(pane, next));
  const edge = pane.grow === 1 ? 'left' : 'right';
  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    const el = e.currentTarget;
    const startX = e.clientX;
    el.setPointerCapture(e.pointerId);
    let last: number | null = null;
    const move = (ev: globalThis.PointerEvent) => {
      last = clamp(pane, width + pane.grow * (ev.clientX - startX));
      el.parentElement!.style.setProperty(pane.cssVar, `${last}px`);
      el.style.setProperty(edge, `${last}px`);
      el.setAttribute('aria-valuenow', String(last));
    };
    el.addEventListener('pointermove', move);
    el.addEventListener(
      'lostpointercapture',
      () => {
        el.removeEventListener('pointermove', move);
        if (last !== null) onCommit(last);
      },
      { once: true },
    );
  };
  const onKeyDown = (e: KeyboardEvent) => {
    const step = e.key === 'ArrowRight' ? KEY_STEP : e.key === 'ArrowLeft' ? -KEY_STEP : 0;
    if (step === 0) return;
    e.preventDefault();
    onCommit(width + pane.grow * step);
  };
  return (
    <div
      className="splitter"
      role="separator"
      aria-orientation="vertical"
      aria-label={props.label}
      aria-valuenow={width}
      aria-valuemin={pane.min}
      aria-valuemax={pane.max}
      tabIndex={0}
      style={{ [edge]: width }}
      onPointerDown={onPointerDown}
      onKeyDown={onKeyDown}
      onDoubleClick={() => onCommit(pane.initial)}
    />
  );
}
