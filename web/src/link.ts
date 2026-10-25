export const RECONNECT_MS = 1000;
export const CONNECT_LIMIT_MS = 5000;
export const QUIET_LIMIT_MS = 40_000;

export interface Link {
  send: (data: string | Uint8Array<ArrayBuffer>) => void;
  close: () => void;
}

export interface LinkHandlers {
  onOpen: () => void;
  onMessage: (e: MessageEvent) => void;
  onDrop?: () => void;
  quietLimitMs?: number;
}

const live = new Set<() => void>();

const restartAll = () => {
  for (const restart of [...live]) restart();
};

export function reconnecting(url: string, handlers: LinkHandlers): Link {
  let socket: WebSocket | undefined;
  let stopped = false;
  let watchdog: ReturnType<typeof setTimeout> | undefined;
  let retry: ReturnType<typeof setTimeout> | undefined;
  let hiddenAt: number | undefined;
  const watch = (ms: number | null, expired: () => void = restartAll) => {
    clearTimeout(watchdog);
    watchdog = ms === null ? undefined : setTimeout(expired, ms);
  };
  const open = () => {
    clearTimeout(retry);
    retry = undefined;
    if (stopped) return;
    const ws = new WebSocket(url);
    socket = ws;
    ws.binaryType = 'arraybuffer';
    ws.onopen = () => {
      watch(handlers.quietLimitMs ?? null);
      handlers.onOpen();
    };
    ws.onmessage = (e) => {
      watch(handlers.quietLimitMs ?? null);
      handlers.onMessage(e);
    };
    ws.onclose = () => {
      if (ws !== socket || stopped) return;
      watch(null);
      socket = undefined;
      handlers.onDrop?.();
      retry = setTimeout(open, RECONNECT_MS);
    };
    watch(CONNECT_LIMIT_MS, restart);
  };
  const abandon = () => {
    const ws = socket;
    socket = undefined;
    watch(null);
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
    if (document.visibilityState === 'hidden') {
      hiddenAt = Date.now();
      return;
    }
    const away = hiddenAt === undefined ? 0 : Date.now() - hiddenAt;
    hiddenAt = undefined;
    if (away >= QUIET_LIMIT_MS) restart();
    else if (retry !== undefined) open();
  };
  const onPageShow = (e: PageTransitionEvent) => {
    if (e.persisted) restart();
  };
  live.add(restart);
  document.addEventListener('visibilitychange', onVisibility);
  window.addEventListener('online', restart);
  window.addEventListener('pageshow', onPageShow);
  open();
  return {
    send: (data) => {
      if (socket?.readyState === WebSocket.OPEN) socket.send(data);
    },
    close: () => {
      stopped = true;
      live.delete(restart);
      clearTimeout(retry);
      document.removeEventListener('visibilitychange', onVisibility);
      window.removeEventListener('online', restart);
      window.removeEventListener('pageshow', onPageShow);
      const ws = socket;
      socket = undefined;
      watch(null);
      ws?.close();
    },
  };
}
