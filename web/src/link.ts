import type { Pace } from './gen/Pace';

export const RECONNECT_MS = 1000;
export const CONNECT_LIMIT_MS = 5000;

export interface Link {
  send: (data: string | Uint8Array<ArrayBuffer>) => void;
  beat: (quietMs: number | undefined) => void;
  close: () => void;
}

export interface LinkHandlers {
  onOpen: () => void;
  onMessage: (e: MessageEvent) => void;
  onDrop?: () => void;
}

export function reconnecting(url: string, handlers: LinkHandlers): Link {
  let socket: WebSocket | undefined;
  let stopped = false;
  let watchdog: ReturnType<typeof setTimeout> | undefined;
  let retry: ReturnType<typeof setTimeout> | undefined;
  let hiddenAt: number | undefined;
  let limit: number | undefined;
  const watch = (ms: number | undefined) => {
    clearTimeout(watchdog);
    watchdog = ms === undefined ? undefined : setTimeout(restart, ms);
  };
  const pace = () => {
    if (socket?.readyState !== WebSocket.OPEN) return;
    socket.send(JSON.stringify({ pace: document.visibilityState === 'hidden' ? 'back' : 'front' } satisfies Pace));
  };
  const open = () => {
    clearTimeout(retry);
    retry = undefined;
    if (stopped) return;
    const ws = new WebSocket(url);
    socket = ws;
    limit = CONNECT_LIMIT_MS;
    ws.binaryType = 'arraybuffer';
    ws.onopen = () => {
      watch(limit);
      pace();
      handlers.onOpen();
    };
    ws.onmessage = (e) => {
      watch(limit);
      handlers.onMessage(e);
    };
    ws.onclose = () => {
      if (ws !== socket || stopped) return;
      watch(undefined);
      socket = undefined;
      handlers.onDrop?.();
      retry = setTimeout(open, RECONNECT_MS);
    };
    watch(limit);
  };
  const abandon = () => {
    const ws = socket;
    socket = undefined;
    watch(undefined);
    if (!ws) return;
    ws.onopen = ws.onmessage = ws.onclose = null;
    ws.close();
    handlers.onDrop?.();
  };
  function restart() {
    abandon();
    open();
  }
  const onVisibility = () => {
    pace();
    if (document.visibilityState === 'hidden') {
      hiddenAt = Date.now();
      return;
    }
    const away = hiddenAt === undefined ? 0 : Date.now() - hiddenAt;
    hiddenAt = undefined;
    if (limit !== undefined && away >= limit) restart();
    else if (retry !== undefined) open();
  };
  const onPageShow = (e: PageTransitionEvent) => {
    if (e.persisted) restart();
  };
  document.addEventListener('visibilitychange', onVisibility);
  window.addEventListener('online', restart);
  window.addEventListener('pageshow', onPageShow);
  open();
  return {
    send: (data) => {
      if (socket?.readyState === WebSocket.OPEN) socket.send(data);
    },
    beat: (ms) => {
      limit = ms;
      watch(limit);
    },
    close: () => {
      stopped = true;
      clearTimeout(retry);
      document.removeEventListener('visibilitychange', onVisibility);
      window.removeEventListener('online', restart);
      window.removeEventListener('pageshow', onPageShow);
      const ws = socket;
      socket = undefined;
      watch(undefined);
      ws?.close();
    },
  };
}
