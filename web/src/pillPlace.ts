import type { PillPlace } from './gen/PillPlace';

/** 알약이 그래프 칸 가장자리에서 떨어지는 간격(px). 칸 위 제목(.canvas-title)과 겹치지 않던 옛 첫 자리(top 12px)와 같다. */
export const PILL_MARGIN = 12;
/** 이 거리(px)보다 적게 움직인 누르기는 끌기가 아니라 누르기다. 알약 머리 단추의 클릭을 지킨다. */
export const DRAG_SLOP = 4;

export type Size = { w: number; h: number };

const unit = (v: number) => Math.min(1, Math.max(0, v));

/** 알약이 움직일 수 있는 폭·높이: 칸에서 알약과 양쪽 간격을 뺀 것. 칸이 알약보다 작으면 0이다. */
const free = (pane: Size, box: Size): Size => ({ w: Math.max(0, pane.w - box.w - 2 * PILL_MARGIN), h: Math.max(0, pane.h - box.h - 2 * PILL_MARGIN) });

/** 알약 왼쪽 위 모서리(칸 안 px). 자리가 없으면(`null`) 위 가운데다. 칸이나 알약 크기가 바뀌어도 칸 안에 남는다. */
export function placeOf(place: PillPlace | null, pane: Size, box: Size): { left: number; top: number } {
  const room = free(pane, box);
  if (place === null) return { left: PILL_MARGIN + room.w / 2, top: PILL_MARGIN };
  return { left: PILL_MARGIN + unit(place.x) * room.w, top: PILL_MARGIN + unit(place.y) * room.h };
}

/** `placeOf`의 거꾸로: 끌어 놓은 왼쪽 위 모서리를 저장할 자리로 바꾼다. 칸 밖으로 끌어도 가장자리에 멈춘다. */
export function placeFrom(left: number, top: number, pane: Size, box: Size): PillPlace {
  const room = free(pane, box);
  return { x: room.w === 0 ? 0 : unit((left - PILL_MARGIN) / room.w), y: room.h === 0 ? 0 : unit((top - PILL_MARGIN) / room.h) };
}

/** 목록 상자의 왼쪽(알약 왼쪽 기준 px). 알약 가운데에 맞추되 칸 밖으로 나가지 않는다. 칸보다 넓으면 왼쪽 간격에 붙인다. */
export function popLeft(pillLeft: number, pillWidth: number, popWidth: number, paneWidth: number): number {
  const centred = pillLeft + (pillWidth - popWidth) / 2;
  const clamped = Math.max(PILL_MARGIN, Math.min(centred, paneWidth - PILL_MARGIN - popWidth));
  return clamped - pillLeft;
}

/** 알약이 칸 아래 절반에 있으면 목록을 알약 위로 연다. 아래로 열면 칸 밖으로 잘린다. */
export const popAbove = (top: number, box: Size, pane: Size): boolean => top + box.h / 2 > pane.h / 2;
