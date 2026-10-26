export type PhoneNotify = { state: () => string; ask: (on: boolean) => void; done: (on: boolean) => void; link?: () => void };
export type PhoneBrowser = { open: (url: string) => void };

declare global {
  interface Window {
    loomClipboard?: { read: () => string };
    loomChrome?: { setScheme: (scheme: string) => void; haptic?: () => void };
    loomBrowser?: PhoneBrowser;
    loomNotify?: PhoneNotify;
  }
}

export const NOTIFY_EVENT = 'loomnotify';
