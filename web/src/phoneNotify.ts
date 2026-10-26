import { useEffect, useState } from 'react';
import { NOTIFY_EVENT, type PhoneNotify } from './native';

export type NotifyState = { ask: boolean; done: boolean; allowed: boolean };

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
