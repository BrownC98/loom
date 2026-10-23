import type { PillPlace } from './gen/PillPlace';

export const PILL_MARGIN = 12;
export const DRAG_SLOP = 4;

export type Size = { w: number; h: number };

const unit = (v: number) => Math.min(1, Math.max(0, v));

const free = (pane: Size, box: Size): Size => ({ w: Math.max(0, pane.w - box.w - 2 * PILL_MARGIN), h: Math.max(0, pane.h - box.h - 2 * PILL_MARGIN) });

export function placeOf(place: PillPlace | null, pane: Size, box: Size): { left: number; top: number } {
  const room = free(pane, box);
  if (place === null) return { left: PILL_MARGIN + room.w / 2, top: PILL_MARGIN };
  return { left: PILL_MARGIN + unit(place.x) * room.w, top: PILL_MARGIN + unit(place.y) * room.h };
}

export function placeFrom(left: number, top: number, pane: Size, box: Size): PillPlace {
  const room = free(pane, box);
  return { x: room.w === 0 ? 0 : unit((left - PILL_MARGIN) / room.w), y: room.h === 0 ? 0 : unit((top - PILL_MARGIN) / room.h) };
}

export function popLeft(pillLeft: number, pillWidth: number, popWidth: number, paneWidth: number): number {
  const centred = pillLeft + (pillWidth - popWidth) / 2;
  const clamped = Math.max(PILL_MARGIN, Math.min(centred, paneWidth - PILL_MARGIN - popWidth));
  return clamped - pillLeft;
}

export const POP_GAP = 6;

export function popSpace(top: number, box: Size, pane: Size): { above: boolean; room: number } {
  const above = top - PILL_MARGIN;
  const below = pane.h - (top + box.h) - PILL_MARGIN;
  const useAbove = above > below;
  return { above: useAbove, room: Math.max(0, (useAbove ? above : below) - POP_GAP) };
}
