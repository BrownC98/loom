/** 질문의 선택지 하나. `key`가 답으로 가는 글이다. */
export type Choice = { key: string; label: string; recommended: boolean };

const OPTION = /^([A-Z]):\s*(.+)$/;
const RECOMMENDED = /^[^:]+:\s*([A-Z])$/;

/**
 * 표준 질문 꼴 `<질문> (A: <선택지> / B: <선택지>, 권장: A)`의 선택지(skills/ask-human). 「권장」 낱말은 사람의 언어라 무엇이든 받는다.
 * 꼴이 아니거나 선택지가 둘보다 적으면 빈 목록이다. 그때 사람은 입력창에만 답을 쓴다.
 */
export function choices(question: string): Choice[] {
  const inside = /\(([^()]*)\)\s*$/.exec(question.trim())?.[1];
  if (inside === undefined) return [];
  const cut = inside.lastIndexOf(',');
  const tail = cut < 0 ? null : RECOMMENDED.exec(inside.slice(cut + 1).trim());
  const list = tail ? inside.slice(0, cut) : inside;
  const options = list.split(' / ').map((part) => OPTION.exec(part.trim()));
  if (options.length < 2 || options.some((m) => m === null)) return [];
  return options.map((m) => ({ key: m![1], label: m![2].trim(), recommended: m![1] === tail?.[1] }));
}
