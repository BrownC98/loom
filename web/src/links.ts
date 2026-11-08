import type { TextPart } from './gen/TextPart';

export function plainText(parts: readonly TextPart[]): string {
  return parts.map((part) => ('label' in part ? part.label : part.text)).join('');
}

export function filePath(url: URL): string {
  const path = decodeURIComponent(url.pathname).replace(/^\/+(?=[A-Za-z]:)/, '/');
  return url.hostname ? `//${url.hostname}${path}` : path;
}
