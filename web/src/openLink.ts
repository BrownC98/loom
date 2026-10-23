// 터미널과 받은편지함의 링크를 그 화면에 맞는 길로 연다: 데몬 PC의 기본 앱, 폰 기본 브라우저, 화면 안 새 탭.
// links.ts(글 나누기)와 따로 둔다. 이 파일은 api.ts를 불러 시험 환경(문서 없음)에서 읽히지 않는다.
import { isTauri } from '@tauri-apps/api/core';
import { filePass, fileViewUrl, openFile } from './api';
import type { NodeId } from './gen/NodeId';

/** 이 화면이 데몬 PC 밖(폰 문, Tailscale)에서 열렸다. 데스크톱 앱과 PC 브라우저는 루프백으로 연다. */
function remoteScreen(): boolean {
  return !isTauri() && !['127.0.0.1', 'localhost', '[::1]'].includes(location.hostname);
}

type PhoneBrowser = { open: (url: string) => void };
/**
 * 안드로이드 앱이 붙이는 다리(`MainActivity`의 `loomBrowser`). 통행증 주소를 폰 기본 브라우저(Chrome)로 연다. 옛 APK에는 없다.
 * `window.open`은 쓰지 않는다. 앱 WebView 안에서 열려 줌이 되지 않았다(2026-10-23 사람 "여전히 헤드리스 브라우저임").
 */
export const phoneBrowser = () => (window as { loomBrowser?: PhoneBrowser }).loomBrowser;

/** 쿠키가 없는 폰 브라우저가 읽도록 통행증 주소를 연다. */
export const openPass = (pass: Promise<string>, browser: PhoneBrowser) =>
  pass.then((url) => browser.open(new URL(url, location.href).href));

/**
 * 터미널과 받은편지함 글의 링크를 연다. `file://`은 데몬이 그 기계의 기본 앱으로 연다(브라우저 페이지는 열지 못한다). `http(s)`는 새 탭이다. 다른 형식은 열지 않는다.
 * Windows 경로(`file:///C:/…`)는 앞의 `/`를 뗀다.
 */
export function openLink(uri: string, node: NodeId | null) {
  const url = URL.parse(uri);
  if (url?.protocol === 'file:') {
    const path = decodeURIComponent(url.pathname);
    const hostPath = /^\/[A-Za-z]:/.test(path) ? path.slice(1) : path;
    const browser = phoneBrowser();
    if (browser) return void openPass(filePass(hostPath, node), browser).catch((err: Error) => console.error(err));
    // 폰 브라우저는 데몬 PC가 아니다. PC에서 기본 앱을 열면 사람이 보지 못하므로 화면 안에서 연다.
    if (remoteScreen()) return openPage(fileViewUrl(path, node));
    openFile(hostPath, node).catch((err: Error) => console.error(err));
  } else if (url?.protocol === 'http:' || url?.protocol === 'https:') {
    window.open(url.href, '_blank', 'noopener');
  }
}

/** 팝업을 막으면 이 탭에서 연다. */
function openPage(href: string) {
  if (!window.open(href, '_blank')) location.assign(href);
}
