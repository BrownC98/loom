import { isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { onAction, sendNotification } from '@tauri-apps/plugin-notification';

export async function requestNotifyPermission(): Promise<boolean> {
  if (typeof Notification === 'undefined') return false;
  if (Notification.permission === 'granted') return true;
  return (await Notification.requestPermission()) === 'granted';
}

export function notifyAllowed(): boolean {
  return typeof Notification !== 'undefined' && Notification.permission === 'granted';
}

export async function windowHidden(): Promise<boolean> {
  if (document.hidden) return true;
  if (!isTauri()) return false;
  try {
    const window = getCurrentWindow();
    return (await window.isMinimized()) || !(await window.isVisible());
  } catch {
    return false;
  }
}

const clicks = new Map<number, () => void>();
let nextId = 1;
let listening: Promise<unknown> | null = null;

export function notify(title: string, body: string, tag: string, onClick: () => void) {
  if (!isTauri()) {
    const note = new Notification(title, { body, tag });
    note.onclick = () => {
      window.focus();
      onClick();
      note.close();
    };
    return;
  }
  listening ??= onAction((event) => {
    const id = event.notification?.id;
    if (id === undefined) return;
    clicks.get(id)?.();
    clicks.delete(id);
  });
  const id = nextId++;
  clicks.set(id, onClick);
  sendNotification({ id, title, body });
}
