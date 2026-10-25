import { useEffect, useState } from 'react';

export type NotifyState = { ask: boolean; done: boolean; allowed: boolean };

export type PhoneNotify = { state: () => string; ask: (on: boolean) => void; done: (on: boolean) => void };

export const NOTIFY_EVENT = 'loomnotify';

export const phoneNotify = () => (window as { loomNotify?: PhoneNotify }).loomNotify;

export function readNotify(bridge: PhoneNotify): NotifyState {
  try {
    const value = JSON.parse(bridge.state()) as Partial<NotifyState>;
    return { ask: value.ask === true, done: value.done === true, allowed: value.allowed === true };
  } catch {
    return { ask: false, done: false, allowed: false };
  }
}

export function useNotifyState(bridge: PhoneNotify): NotifyState {
  const [state, setState] = useState(() => readNotify(bridge));
  useEffect(() => {
    const sync = () => setState(readNotify(bridge));
    window.addEventListener(NOTIFY_EVENT, sync);
    return () => window.removeEventListener(NOTIFY_EVENT, sync);
  }, [bridge]);
  return state;
}
