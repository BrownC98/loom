import { isTauri } from '@tauri-apps/api/core';
import { filePass, fileViewUrl, openFile } from './api';
import type { NodeId } from './gen/NodeId';
import { filePath } from './links';
import type { PhoneBrowser } from './native';

function remoteScreen(): boolean {
  return !isTauri() && !['127.0.0.1', 'localhost', '[::1]'].includes(location.hostname);
}

export const openPass = (pass: Promise<string>, browser: PhoneBrowser) =>
  pass.then((url) => browser.open(new URL(url, location.href).href));

export function openLink(uri: string, node: NodeId | null) {
  const url = URL.parse(uri);
  if (url?.protocol === 'file:') {
    const path = filePath(url);
    const hostPath = /^\/[A-Za-z]:/.test(path) ? path.slice(1) : path;
    const browser = window.loomBrowser;
    if (browser) return void openPass(filePass(hostPath, node), browser).catch((err: Error) => console.error(err));
    if (remoteScreen()) return openPage(fileViewUrl(path, node));
    openFile(hostPath, node).catch((err: Error) => console.error(err));
  } else if (url?.protocol === 'http:' || url?.protocol === 'https:') {
    window.open(url.href, '_blank', 'noopener');
  }
}

function openPage(href: string) {
  if (!window.open(href, '_blank')) location.assign(href);
}
