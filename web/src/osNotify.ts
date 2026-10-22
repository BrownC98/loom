import { isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { onAction, sendNotification } from '@tauri-apps/plugin-notification';

/**
 * 시스템 알림. 브라우저는 Web Notification을 쓴다.
 * 앱(WebView2·WKWebView)은 Web Notification을 막거나 없앤다. 앱의 `tauri-plugin-notification`이 `window.Notification`을 OS 알림으로 바꾸고, 허용을 묻지 않는다.
 * 그 플러그인이 없는 옛 앱 판에서는 허용이 `denied`로 온다. 근거: docs/design.md 「시스템 알림」.
 */

/** 설정을 켤 때 부른다(사람의 클릭). 허용되면 true. */
export async function requestNotifyPermission(): Promise<boolean> {
  if (typeof Notification === 'undefined') return false;
  if (Notification.permission === 'granted') return true;
  return (await Notification.requestPermission()) === 'granted';
}

export function notifyAllowed(): boolean {
  return typeof Notification !== 'undefined' && Notification.permission === 'granted';
}

/** WebView2가 최소화한 창에서 `document.hidden`을 바꾸는지 확인하지 못했다. 앱은 창 상태를 직접 묻는다. */
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

/** 앱 알림 번호 → 눌렀을 때 할 일. 앱이 창을 앞으로 가져온다(app/src/main.rs). 화면은 노드만 고른다. */
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
