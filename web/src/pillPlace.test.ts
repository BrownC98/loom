import { describe, expect, it } from 'vitest';
import { PILL_MARGIN, placeFrom, placeOf, popAbove, popLeft } from './pillPlace';

const pane = { w: 1000, h: 600 };
const box = { w: 200, h: 40 };

describe('placeOf', () => {
  it('puts the default place at top centre', () => {
    expect(placeOf(null, pane, box)).toEqual({ left: 400, top: PILL_MARGIN });
  });
  it('maps the saved fractions onto the free room', () => {
    expect(placeOf({ x: 0, y: 0 }, pane, box)).toEqual({ left: PILL_MARGIN, top: PILL_MARGIN });
    expect(placeOf({ x: 1, y: 1 }, pane, box)).toEqual({ left: 1000 - 200 - PILL_MARGIN, top: 600 - 40 - PILL_MARGIN });
  });
  it('keeps the pill inside when the pane shrinks, and tolerates a bad saved value', () => {
    const small = { w: 300, h: 100 };
    const got = placeOf({ x: 1, y: 1 }, small, box);
    expect(got.left + box.w).toBeLessThanOrEqual(small.w - PILL_MARGIN);
    expect(got.top + box.h).toBeLessThanOrEqual(small.h - PILL_MARGIN);
    expect(placeOf({ x: 9, y: -3 }, pane, box)).toEqual(placeOf({ x: 1, y: 0 }, pane, box));
  });
  it('does not go negative when the pane is smaller than the pill', () => {
    expect(placeOf({ x: 0.5, y: 0.5 }, { w: 100, h: 20 }, box)).toEqual({ left: PILL_MARGIN, top: PILL_MARGIN });
  });
});

describe('placeFrom', () => {
  it('inverts placeOf', () => {
    const place = { x: 0.25, y: 0.5 };
    const at = placeOf(place, pane, box);
    expect(placeFrom(at.left, at.top, pane, box)).toEqual(place);
  });
  it('stops at the pane edge when dragged outside', () => {
    expect(placeFrom(-500, 5000, pane, box)).toEqual({ x: 0, y: 1 });
    expect(placeFrom(0, 0, { w: 100, h: 20 }, box)).toEqual({ x: 0, y: 0 });
  });
});

describe('popLeft', () => {
  it('centres the list under the pill when it fits', () => {
    expect(popLeft(400, 200, 520, 1000)).toBe(-160);
  });
  it('slides the list back inside near the edges', () => {
    expect(popLeft(PILL_MARGIN, 200, 520, 1000)).toBe(0);
    const right = 1000 - PILL_MARGIN - 200;
    expect(right + popLeft(right, 200, 520, 1000) + 520).toBe(1000 - PILL_MARGIN);
  });
});

describe('popAbove', () => {
  it('opens upward from the lower half', () => {
    expect(popAbove(PILL_MARGIN, box, pane)).toBe(false);
    expect(popAbove(500, box, pane)).toBe(true);
  });
});
