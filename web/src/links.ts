/** 붙은 문서를 가리키는 링크 주소. CLI가 `--doc` 파일에 건 링크를 이 주소로 바꾼다(`src/user_doc.rs` `DOC_LINK`). */
export const DOC_LINK = 'loom:doc';

const LINK = new RegExp(String.raw`\[([^\]\n]+)\]\(((?:file|https?):\/\/[^\s)]+|${DOC_LINK})\)`, 'g');

export type TextPart = { text: string } | { label: string; href: string };

/**
 * 에이전트가 쓴 글의 markdown 링크 `[이름](주소)`를 나눈다. 다른 markdown은 그대로 글이다.
 * 공용 지시가 사람에게 보이는 파일·주소를 markdown 링크로 쓰게 한다. 받은편지함은 글을 그대로 보여 링크가 글자로 깨졌다(2026-10-23 사람 "방금 답변 링크 깨짐").
 */
export function linkParts(text: string): TextPart[] {
  const parts: TextPart[] = [];
  let at = 0;
  for (const m of text.matchAll(LINK)) {
    if (m.index > at) parts.push({ text: text.slice(at, m.index) });
    parts.push({ label: m[1], href: m[2] });
    at = m.index + m[0].length;
  }
  if (at < text.length) parts.push({ text: text.slice(at) });
  return parts;
}

/** 링크를 이름만 남긴 글. 한 줄 미리보기와 OS 알림처럼 링크를 누를 수 없는 곳에 쓴다. */
export function plainText(text: string): string {
  return linkParts(text)
    .map((part) => ('href' in part ? part.label : part.text))
    .join('');
}
