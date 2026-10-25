import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { CONNECT_LIMIT_MS, QUIET_LIMIT_MS, RECONNECT_MS, reconnecting } from './link';

class FakeSocket {
  static OPEN = 1;
  static made: FakeSocket[] = [];
  readyState = 0;
  binaryType = '';
  closed = false;
  onopen: (() => void) | null = null;
  onmessage: ((e: MessageEvent) => void) | null = null;
  onclose: (() => void) | null = null;
  constructor(readonly url: string) {
    FakeSocket.made.push(this);
  }
  send() {}
  close() {
    this.closed = true;
  }
  accept() {
    this.readyState = FakeSocket.OPEN;
    this.onopen?.();
  }
  say(data: string) {
    this.onmessage?.({ data } as MessageEvent);
  }
  drop() {
    this.readyState = 3;
    this.onclose?.();
  }
}

const page = { visibilityState: 'visible', events: new EventTarget() };

const latest = () => FakeSocket.made[FakeSocket.made.length - 1];

function show(state: 'visible' | 'hidden') {
  page.visibilityState = state;
  page.events.dispatchEvent(new Event('visibilitychange'));
}

const links: { close: () => void }[] = [];

function link() {
  const seen = { opens: 0, drops: 0 };
  const handle = reconnecting('ws://pc/api/events', {
    onOpen: () => seen.opens++,
    onMessage: () => {},
    onDrop: () => seen.drops++,
    quietLimitMs: QUIET_LIMIT_MS,
  });
  links.push(handle);
  return { seen, handle };
}

const win = new EventTarget();

beforeEach(() => {
  vi.useFakeTimers();
  FakeSocket.made = [];
  page.visibilityState = 'visible';
  vi.stubGlobal('WebSocket', FakeSocket);
  vi.stubGlobal('document', {
    get visibilityState() {
      return page.visibilityState;
    },
    addEventListener: page.events.addEventListener.bind(page.events),
    removeEventListener: page.events.removeEventListener.bind(page.events),
  });
  vi.stubGlobal('window', win);
});

afterEach(() => {
  for (const handle of links.splice(0)) handle.close();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

test('a dropped socket opens again after the retry delay', () => {
  const { seen } = link();
  latest().accept();
  latest().drop();
  expect(seen.drops).toBe(1);
  vi.advanceTimersByTime(RECONNECT_MS - 1);
  expect(FakeSocket.made).toHaveLength(1);
  vi.advanceTimersByTime(1);
  expect(FakeSocket.made).toHaveLength(2);
});

test('a socket that hears no beat for the quiet limit opens again', () => {
  const { seen } = link();
  latest().accept();
  for (let t = 0; t < 4; t++) {
    vi.advanceTimersByTime(15_000);
    latest().say('{"kind":"beat"}');
  }
  expect(FakeSocket.made).toHaveLength(1);
  vi.advanceTimersByTime(QUIET_LIMIT_MS);
  expect(FakeSocket.made[0].closed).toBe(true);
  expect(FakeSocket.made).toHaveLength(2);
  expect(seen.drops).toBe(1);
  FakeSocket.made[0].drop();
  expect(seen.drops).toBe(1);
});

test('a quiet beat link also opens the links without beats again', () => {
  link();
  latest().accept();
  const pty = reconnecting('ws://pc/api/nodes/1/pty', { onOpen: () => {}, onMessage: () => {} });
  links.push(pty);
  latest().accept();
  vi.advanceTimersByTime(QUIET_LIMIT_MS);
  expect(FakeSocket.made.slice(0, 2).every((s) => s.closed)).toBe(true);
  expect(FakeSocket.made.slice(2).map((s) => s.url).sort()).toEqual(['ws://pc/api/events', 'ws://pc/api/nodes/1/pty']);
});

test('a connection attempt that hangs is given up and tried again', () => {
  link();
  vi.advanceTimersByTime(CONNECT_LIMIT_MS);
  expect(FakeSocket.made[0].closed).toBe(true);
  expect(FakeSocket.made).toHaveLength(2);
});

test('a page shown after a long time hidden opens a new socket at once', () => {
  link();
  latest().accept();
  show('hidden');
  vi.setSystemTime(Date.now() + QUIET_LIMIT_MS);
  show('visible');
  expect(FakeSocket.made).toHaveLength(2);
});

test('a page shown after a short time keeps its socket', () => {
  link();
  latest().accept();
  show('hidden');
  vi.setSystemTime(Date.now() + 1000);
  show('visible');
  expect(FakeSocket.made).toHaveLength(1);
});

test('a page shown while it waits to retry retries at once', () => {
  link();
  latest().drop();
  show('hidden');
  show('visible');
  expect(FakeSocket.made).toHaveLength(2);
});

test('the network coming back opens a new socket', () => {
  link();
  latest().accept();
  win.dispatchEvent(new Event('online'));
  expect(FakeSocket.made).toHaveLength(2);
});

test('a closed link does not open again', () => {
  const { handle } = link();
  latest().accept();
  handle.close();
  latest().drop();
  win.dispatchEvent(new Event('online'));
  vi.advanceTimersByTime(QUIET_LIMIT_MS * 2);
  expect(FakeSocket.made).toHaveLength(1);
});
