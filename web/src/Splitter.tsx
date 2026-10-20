import { useState, type CSSProperties, type KeyboardEvent, type PointerEvent } from 'react';

/**
 * 너비를 바꿀 수 있는 사이드바(목업 docs/mockups/sidebar-resize.html).
 * `grow`: 경계가 오른쪽으로 1px 갈 때 늘어나는 너비. 왼쪽 칸은 1, 오른쪽 칸은 -1이다.
 */
export type Pane = { key: string; initial: number; min: number; max: number; grow: 1 | -1 };

/** ←·→ 한 번에 바꾸는 너비. */
const KEY_STEP = 16;

const clamp = (pane: Pane, width: number) => Math.min(pane.max, Math.max(pane.min, Math.round(width)));

/** 사이드바 너비. 이 브라우저에만 남긴다. 저장소를 쓸 수 없으면(사생활 보호 창 등) 기본 너비를 쓴다. */
export function usePaneWidth(pane: Pane): [number, (width: number) => void] {
  const [width, setWidth] = useState(() => {
    try {
      const saved = Number(localStorage.getItem(pane.key));
      return saved ? clamp(pane, saved) : pane.initial;
    } catch {
      return pane.initial;
    }
  });
  const set = (next: number) => {
    const w = clamp(pane, next);
    setWidth(w);
    try {
      localStorage.setItem(pane.key, String(w));
    } catch {
      // 저장하지 못해도 이번 창에서는 바뀐 너비를 쓴다.
    }
  };
  return [width, set];
}

/** 사이드바 경계. 끌거나 ←·→로 너비를 바꾸고, 두 번 누르면 기본 너비로 돌린다. */
export function Splitter(props: { pane: Pane; width: number; onWidth: (width: number) => void; label: string; style: CSSProperties }) {
  const { pane, width, onWidth } = props;
  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    // 두 번 누르면 옆 글자가 선택된다. 그 위에서 끌기를 시작하면 브라우저가 글자 끌기로 바꾸고 포인터 캡처를 끊는다. 그래서 기본 동작을 막는다.
    e.preventDefault();
    const el = e.currentTarget;
    const startX = e.clientX;
    el.setPointerCapture(e.pointerId);
    const move = (ev: globalThis.PointerEvent) => onWidth(width + pane.grow * (ev.clientX - startX));
    el.addEventListener('pointermove', move);
    el.addEventListener('lostpointercapture', () => el.removeEventListener('pointermove', move), { once: true });
  };
  const onKeyDown = (e: KeyboardEvent) => {
    const step = e.key === 'ArrowRight' ? KEY_STEP : e.key === 'ArrowLeft' ? -KEY_STEP : 0;
    if (step === 0) return;
    e.preventDefault();
    onWidth(width + pane.grow * step);
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
      style={props.style}
      onPointerDown={onPointerDown}
      onKeyDown={onKeyDown}
      onDoubleClick={() => onWidth(pane.initial)}
    />
  );
}
