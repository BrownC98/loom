import { useEffect, useState, type KeyboardEvent, type PointerEvent } from 'react';
import type { PaneWidths } from './gen/PaneWidths';

/**
 * 너비를 바꿀 수 있는 사이드바(목업 docs/mockups/sidebar-resize.html).
 * `grow`: 경계가 오른쪽으로 1px 갈 때 늘어나는 너비. 왼쪽 칸은 1, 오른쪽 칸은 -1이다.
 * `cssVar`: 칸 너비를 담는 `.app`의 CSS 변수(styles.css `@property`).
 */
export type Pane = { key: keyof PaneWidths; cssVar: '--side-w' | '--dock-w'; initial: number; min: number; max: number; grow: 1 | -1 };

/** ←·→ 한 번에 바꾸는 너비. */
const KEY_STEP = 16;

const clamp = (pane: Pane, width: number) => Math.min(pane.max, Math.max(pane.min, Math.round(width)));

/** 창 폭. 창 크기가 바뀌면 다시 잰다. 오른쪽 칸의 너비 상한을 창 안으로 두는 데 쓴다. */
export function useViewportWidth(): number {
  const [width, setWidth] = useState(() => window.innerWidth);
  useEffect(() => {
    const onResize = () => setWidth(window.innerWidth);
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, []);
  return width;
}

/** 사이드바 너비. 저장한 값(`saved`)은 데몬 설정(`PaneWidths`)에 있다. 상한이 줄면(창을 좁히면) 너비도 그 안으로 당긴다. */
export const paneWidth = (pane: Pane, saved: number | null) => clamp(pane, saved ?? pane.initial);

/**
 * 사이드바 경계. 끌거나 ←·→로 너비를 바꾸고, 두 번 누르면 기본 너비로 돌린다.
 * 끄는 동안은 React 상태를 바꾸지 않고 부모(`.app`)의 `pane.cssVar`와 경계 자리만 바꾼다. 놓을 때 한 번 저장한다(`onCommit`).
 * 상태로 바꾸면 끌기 한 번마다 App 전체(그래프·커밋 그래프)를 다시 그려 프레임이 밀렸다(2026-10-22 사람 보고 "잔랙", 끌기 한 번에 약 3.6만 fiber).
 */
export function Splitter(props: { pane: Pane; width: number; onCommit: (width: number) => void; label: string }) {
  const { pane, width } = props;
  const onCommit = (next: number) => props.onCommit(clamp(pane, next));
  const edge = pane.grow === 1 ? 'left' : 'right';
  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    // 두 번 누르면 옆 글자가 선택된다. 그 위에서 끌기를 시작하면 브라우저가 글자 끌기로 바꾸고 포인터 캡처를 끊는다. 그래서 기본 동작을 막는다.
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
