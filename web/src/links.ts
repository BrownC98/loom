import type { TextPart } from './gen/TextPart';

/** 링크를 이름만 남긴 글. 한 줄 미리보기와 OS 알림처럼 링크를 누를 수 없는 곳에 쓴다. */
export function plainText(parts: readonly TextPart[]): string {
  return parts.map((part) => (part.part === 'text' ? part.text : part.label)).join('');
}

/**
 * `file://` 주소가 가리키는 경로. 호스트가 있으면 UNC 경로(`//wsl.localhost/…`)다. 버리면 다른 경로가 된다.
 * 드라이브 앞의 겹친 `/`만 하나로 줄인다(`file:////C:/…`는 `/C:/…`). 호스트 없는 UNC(`file:////wsl.localhost/…`)는 `//`를 지킨다.
 */
export function filePath(url: URL): string {
  const path = decodeURIComponent(url.pathname).replace(/^\/+(?=[A-Za-z]:)/, '/');
  return url.hostname ? `//${url.hostname}${path}` : path;
}
