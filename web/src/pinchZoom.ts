export type View = { x: number; y: number; zoom: number };
export type Point = { x: number; y: number };

/**
 * 두 손가락 몸짓의 뷰포트. 시작 때 두 손가락 가운데 아래에 있던 그래프 점이 지금 가운데 아래에 오고, 배율은 손가락 사이 거리 비율을 따른다.
 * 그래서 확대·축소와 두 손가락 끌기가 한 식이다. 좌표는 그래프 칸 왼쪽 위 모서리 기준이다.
 */
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

const gapOf = (t: TouchList) =>
  Math.hypot(t[0].clientX - t[1].clientX, t[0].clientY - t[1].clientY);

/**
 * 그래프 안 어디서든(바탕과 카드 위) 두 손가락 핀치로 그래프를 확대·축소한다.
 * React Flow의 핀치는 두 번째 손가락이 카드에 닿으면 받지 못한다: 카드의 끌기 처리기(d3-drag)가 닿기를 가로채 확대 처리기(d3-zoom)에 전하지 않는다
 * (2026-10-23 사람 "지금 화면을 가로 모드로 하면은 그래프가 보이는데 ... 손가락 핀치를 통해서 그래프 줌인 줌 아웃이 가능하도록 해줘").
 * 그래서 두 손가락 움직임은 root에서 먼저 받아 직접 뷰포트를 정하고, 아래(카드 끌기·터미널·React Flow)로 내려보내지 않는다. 닿기는 내려보낸다: 터미널의 길게 누르기 시간을 두 번째 손가락이 끝낸다.
 */
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

  root.addEventListener("touchstart", onStart, {
    capture: true,
    passive: true,
  });
  root.addEventListener("touchmove", onMove, { capture: true, passive: false });
  root.addEventListener("touchend", onEnd, { capture: true, passive: true });
  root.addEventListener("touchcancel", onEnd, { capture: true, passive: true });
  return () => {
    root.removeEventListener("touchstart", onStart, { capture: true });
    root.removeEventListener("touchmove", onMove, { capture: true });
    root.removeEventListener("touchend", onEnd, { capture: true });
    root.removeEventListener("touchcancel", onEnd, { capture: true });
  };
}
