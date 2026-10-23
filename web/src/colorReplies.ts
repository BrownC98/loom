const COLOR_REPLY = /\x1b\](?:10|11|4);[^\x07\x1b]*(?:\x07|\x1b\\)/g;

export function withoutColorReplies(data: string): string {
  return data.replace(COLOR_REPLY, '');
}
