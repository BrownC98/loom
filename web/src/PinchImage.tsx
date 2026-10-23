import { useEffect, useRef, useState } from 'react';
import { attachPinchZoom, clampToBox, type View } from './pinchZoom';

const FIT: View = { x: 0, y: 0, zoom: 1 };
const LIMITS = { min: 1, max: 6 };
/** 이만큼(px) 안 움직인 손가락은 끌기가 아니라 누르기다. */
const TAP_SLOP = 8;

/**
 * 크게 보기 그림. 두 손가락 핀치로 1배~6배 확대하고(그래프와 같은 `pinchView`), 확대한 뒤에는 손가락 하나 끌기로 옮긴다.
 * 누르기: 1배에서는 `onTap`(닫기), 확대한 상태에서는 1배로 되돌린다(2026-10-23 사람 지시 "앱화면 알림 썸네일 크게보기에서 핀치로 추가 줌 가능하게해줘").
 * 안드로이드 WebView가 쪽 확대를 막을 수 있어 그림에 직접 transform을 건다.
 */
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
      // 창은 React 트리에서 알림 줄 안에 있다. 누르기가 줄까지 올라가면 노드 화면으로 옮겨 간다.
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
