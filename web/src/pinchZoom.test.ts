import { describe, expect, it } from "vitest";
import { pinchView } from "./pinchZoom";

const limits = { min: 0.1, max: 3.5 };

describe("pinchView", () => {
  it("keeps the graph point under the fingers when spreading", () => {
    const start = { x: 10, y: 20, zoom: 1 };
    const mid = { x: 100, y: 80 };
    const next = pinchView(start, mid, 50, mid, 100, limits);
    expect(next.zoom).toBe(2);
    // graph point under the middle: (90, 60) -> same screen point
    expect(next.x + 90 * next.zoom).toBe(100);
    expect(next.y + 60 * next.zoom).toBe(80);
  });
  it("pans when the middle moves and the gap holds", () => {
    const next = pinchView(
      { x: 0, y: 0, zoom: 1 },
      { x: 50, y: 50 },
      40,
      { x: 70, y: 40 },
      40,
      limits,
    );
    expect(next).toEqual({ x: 20, y: -10, zoom: 1 });
  });
  it("clamps zoom", () => {
    expect(
      pinchView(
        { x: 0, y: 0, zoom: 1 },
        { x: 0, y: 0 },
        10,
        { x: 0, y: 0 },
        1000,
        limits,
      ).zoom,
    ).toBe(3.5);
    expect(
      pinchView(
        { x: 0, y: 0, zoom: 1 },
        { x: 0, y: 0 },
        1000,
        { x: 0, y: 0 },
        1,
        limits,
      ).zoom,
    ).toBe(0.1);
  });
});
