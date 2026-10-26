import type { KeyboardEvent as ReactKeyboardEvent } from 'react';
import { commandKey } from './platform';

export type Direction = 'left' | 'right' | 'up' | 'down';

const NAV_KEYS: Record<string, Direction> = {
  ArrowLeft: 'left',
  ArrowRight: 'right',
  ArrowUp: 'up',
  ArrowDown: 'down',
};

export function navDirection(e: KeyboardEvent | ReactKeyboardEvent): Direction | null {
  if (!commandKey(e) || !e.shiftKey || e.altKey) return null;
  return NAV_KEYS[e.key] ?? null;
}

export type PaneSide = 'left' | 'right';
const PANE_KEYS: Record<string, PaneSide> = { BracketLeft: 'left', BracketRight: 'right' };

export function paneKey(e: KeyboardEvent | ReactKeyboardEvent): PaneSide | null {
  if (!commandKey(e) || !e.shiftKey || e.altKey) return null;
  return PANE_KEYS[e.code] ?? null;
}

export function findKey(e: KeyboardEvent | ReactKeyboardEvent): boolean {
  return commandKey(e) && e.shiftKey && !e.altKey && e.code === 'KeyF';
}
