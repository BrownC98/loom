import { useEffect, useRef, useState } from 'react';
import { attachPinchZoom, clampToBox, type View } from './pinchZoom';

const FIT: View = { x: 0, y: 0, zoom: 1 };
const LIMITS = { min: 1, max: 6 };
const TAP_SLOP = 8;

export function PinchImage(props: { src: string; alt: string; onTap: () => void }) {
  const root = useRef<HTMLDivElement>(null);
  const view = useRef<View>(FIT);
  const [shown, setShown] = useState<View>(FIT);
  const drag = useRef<{ x: number; y: number; from: View } | null>(null);
  const moved = useRef(false);

  const set = (v: View) => {
    const fitted = clampToBox(v, root.current!.getBoundingClientRect());
    view.current = fitted;
    setShown(fitted);
  };

  useEffect(
    () =>
      attachPinchZoom(root.current!, { get: () => view.current, set }, LIMITS),
    [],
  );

  return (
    <div
      ref={root}
      className="pinch-image"
      onTouchStart={(e) => {
        moved.current = false;
        const t = e.touches[0];
        drag.current = e.touches.length === 1 ? { x: t.clientX, y: t.clientY, from: view.current } : null;
      }}
      onTouchMove={(e) => {
        const t = e.touches[0];
        if (e.touches.length !== 1 || !drag.current) return;
        const dx = t.clientX - drag.current.x;
        const dy = t.clientY - drag.current.y;
        if (Math.hypot(dx, dy) > TAP_SLOP) moved.current = true;
        if (view.current.zoom > 1) set({ ...drag.current.from, x: drag.current.from.x + dx, y: drag.current.from.y + dy });
      }}
      onTouchEnd={(e) => {
        if (e.touches.length > 0) moved.current = true;
        drag.current = null;
      }}
      onClick={(e) => {
        e.stopPropagation();
        if (moved.current) return;
        if (view.current.zoom > 1) set(FIT);
        else props.onTap();
      }}
    >
      <img src={props.src} alt={props.alt} style={{ transform: `translate(${shown.x}px, ${shown.y}px) scale(${shown.zoom})` }} />
    </div>
  );
}
