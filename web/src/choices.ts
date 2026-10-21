/** 질문의 선택지 하나. `key`가 답으로 가는 글이다. */
export type Choice = { key: string; label: string; recommended: boolean };

const OPTION = /^([A-Z]):\s*(.+)$/;
const RECOMMENDED = /^[^:]+:\s*([A-Z])$/;

/**
 * 표준 질문 꼴 `<질문> (A: <선택지> / B: <선택지>, 권장: A)`의 선택지(skills/ask-human). 「권장」 낱말은 사람의 언어라 무엇이든 받는다.
 * 선택지는 질문 끝의 맨 바깥 괄호 묶음이다. 그 안의 괄호(`A: 지금 (빠름)`)는 선택지 글의 일부다.
 * 꼴이 아니거나 선택지가 둘보다 적으면 빈 목록이다. 그때 사람은 입력창에만 답을 쓴다.
 */
export function choices(question: string): Choice[] {
  const inside = lastGroup(question.trim());
  if (inside === null) return [];
  const commas = topLevel(inside, ',');
  const cut = commas.length === 0 ? -1 : commas[commas.length - 1];
  const tail = cut < 0 ? null : RECOMMENDED.exec(inside.slice(cut + 1).trim());
  const list = tail ? inside.slice(0, cut) : inside;
  const options = split(list, ' / ').map((part) => OPTION.exec(part.trim()));
  if (options.length < 2 || options.some((m) => m === null)) return [];
  return options.map((m) => ({ key: m![1], label: m![2].trim(), recommended: m![1] === tail?.[1] }));
}

/** 글 끝의 맨 바깥 `(...)` 안쪽. 글이 `)`로 끝나지 않거나 괄호 짝이 맞지 않으면 없다. */
function lastGroup(text: string): string | null {
  if (!text.endsWith(')')) return null;
  let depth = 0;
  for (let i = text.length - 1; i >= 0; i--) {
    if (text[i] === ')') depth++;
    else if (text[i] === '(' && --depth === 0) return text.slice(i + 1, -1);
  }
  return null;
}

/** 괄호 밖에 있는 `sep`의 시작 위치들. */
function topLevel(text: string, sep: string): number[] {
  const at: number[] = [];
  let depth = 0;
  for (let i = 0; i < text.length; i++) {
    if (text[i] === '(') depth++;
    else if (text[i] === ')') depth--;
    else if (depth === 0 && text.startsWith(sep, i)) at.push(i);
  }
  return at;
}

/** 괄호 밖의 `sep`으로 나눈다. */
function split(text: string, sep: string): string[] {
  const parts: string[] = [];
  let from = 0;
  for (const i of topLevel(text, sep)) {
    parts.push(text.slice(from, i));
    from = i + sep.length;
  }
  parts.push(text.slice(from));
  return parts;
}
