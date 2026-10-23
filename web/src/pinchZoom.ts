export type View = { x: number; y: number; zoom: number };
export type Point = { x: number; y: number };

export function pinchView(
  start: View,
  startMid: Point,
  startGap: number,
  mid: Point,
  gap: number,
  limits: { min: number; max: number },
): View {
  const zoom = Math.min(
    limits.max,
    Math.max(limits.min, start.zoom * (gap / startGap)),
  );
  const anchorX = (startMid.x - start.x) / start.zoom;
  const anchorY = (startMid.y - start.y) / start.zoom;
  return { x: mid.x - anchorX * zoom, y: mid.y - anchorY * zoom, zoom };
}

export function clampToBox(v: View, box: { width: number; height: number }): View {
  return {
    zoom: v.zoom,
    x: Math.min(0, Math.max(box.width * (1 - v.zoom), v.x)),
    y: Math.min(0, Math.max(box.height * (1 - v.zoom), v.y)),
  };
}

const gapOf = (t: TouchList) =>
  Math.hypot(t[0].clientX - t[1].clientX, t[0].clientY - t[1].clientY);

export function attachPinchZoom(
  root: HTMLElement,
  view: { get: () => View; set: (v: View) => void },
  limits: { min: number; max: number },
): () => void {
  let from: { view: View; mid: Point; gap: number } | null = null;
  const rect = () => root.getBoundingClientRect();
  const midOf = (t: TouchList): Point => ({
    x: (t[0].clientX + t[1].clientX) / 2 - rect().left,
    y: (t[0].clientY + t[1].clientY) / 2 - rect().top,
  });

  const onStart = (e: TouchEvent) => {
    if (e.touches.length === 2)
      from = { view: view.get(), mid: midOf(e.touches), gap: gapOf(e.touches) };
  };
  const onMove = (e: TouchEvent) => {
    if (!from || e.touches.length < 2) return;
    e.preventDefault();
    e.stopPropagation();
    view.set(
      pinchView(
        from.view,
        from.mid,
        from.gap,
        midOf(e.touches),
        gapOf(e.touches),
        limits,
      ),
    );
  };
  const onEnd = (e: TouchEvent) => {
    if (e.touches.length < 2) from = null;
  };

  root.addEventListener('touchstart', onStart, {
    capture: true,
    passive: true,
  });
  root.addEventListener('touchmove', onMove, { capture: true, passive: false });
  root.addEventListener('touchend', onEnd, { capture: true, passive: true });
  root.addEventListener('touchcancel', onEnd, { capture: true, passive: true });
  return () => {
    root.removeEventListener('touchstart', onStart, { capture: true });
    root.removeEventListener('touchmove', onMove, { capture: true });
    root.removeEventListener('touchend', onEnd, { capture: true });
    root.removeEventListener('touchcancel', onEnd, { capture: true });
  };
}
