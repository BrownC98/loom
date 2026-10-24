// Side-effect import for the jsdom component tests: browser stubs and per-test cleanup.
import { cleanup } from '@testing-library/react';
import { afterEach, vi } from 'vitest';

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
window.matchMedia ??= ((query: string) => ({ matches: false, media: query, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, onchange: null, dispatchEvent: () => false })) as typeof window.matchMedia;
window.requestAnimationFrame = (cb) => window.setTimeout(() => cb(0), 0);
HTMLCanvasElement.prototype.getContext = () => null;
