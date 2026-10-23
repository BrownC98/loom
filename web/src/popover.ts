// 팝오버·메뉴·시트를 닫는 길: 밖 누르기와 Esc. 근거: 목업 docs/mockups/fragment-std.html ②(2026-10-23 승인 Q503 A).
// Esc는 포커스가 어디 있든 맨 위 하나만 닫는다. 전에는 팝오버마다 그 안에 포커스가 있을 때만 닫거나 문서 전체에서 닫았다.
import { useEffect, useRef, type RefObject } from 'react';

type Open = { box: RefObject<HTMLElement | null>; close: RefObject<() => void> };

/** 열린 순서. 끝이 맨 위다. React는 자식 효과를 먼저 돌리므로 감싼 팝오버가 안쪽보다 위다. 둘 다 같은 것을 닫는다. */
const opened: Open[] = [];

// 캡처 단계에서 받는다. 터미널(xterm)과 그래프가 키를 먼저 먹기 전이다.
document.addEventListener(
  'keydown',
  (e) => {
    if (e.key !== 'Escape') return;
    const top = opened.at(-1);
    const box = top?.box.current;
    if (!top || !box) return;
    // 팝오버 위에 뜬 창(`Modal`)은 브라우저가 Esc로 닫는다. 창 안의 팝오버(설정의 색 고르기)는 팝오버가 먼저다.
    const modal = document.querySelector('dialog:modal');
    if (modal && !modal.contains(box)) return;
    // preventDefault는 창의 `cancel`도 막는다. 팝오버 하나만 닫힌다.
    e.preventDefault();
    e.stopPropagation();
    top.close.current();
  },
  true,
);

/** 돌려준 ref를 단 요소가 떠 있는 동안 Esc가 `onClose`를 부른다. 바깥 누르기를 스스로 맡는 시트(바탕)와 새 노드 창이 쓴다. */
export function useEscapeClose<T extends HTMLElement>(onClose: () => void) {
  const box = useRef<T>(null);
  useEscapeEntry(box, onClose);
  return box;
}

/** 돌려준 ref를 단 요소 밖을 누르면 `onClose`를 부른다. */
export function useOutsideClose<T extends HTMLElement>(onClose: () => void) {
  const box = useRef<T>(null);
  useOutsideEntry(box, onClose, undefined);
  return box;
}

/**
 * 팝오버와 메뉴의 닫기: 밖 누르기와 Esc. 돌려준 ref를 팝오버 상자에 단다.
 * `anchor`(연 버튼)가 있으면 그 버튼을 누르는 것은 밖이 아니다(버튼이 스스로 여닫는다). Esc로 닫으면 포커스가 그 버튼으로 돌아간다.
 */
export function useDismiss<T extends HTMLElement>(onClose: () => void, options: { anchor?: RefObject<HTMLElement | null> } = {}) {
  const box = useRef<T>(null);
  useOutsideEntry(box, onClose, options.anchor);
  useEscapeEntry(box, () => {
    onClose();
    options.anchor?.current?.focus();
  });
  return box;
}

function useEscapeEntry(box: RefObject<HTMLElement | null>, onClose: () => void) {
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    const entry: Open = { box, close };
    opened.push(entry);
    return () => void opened.splice(opened.indexOf(entry), 1);
  }, []);
}

function useOutsideEntry(box: RefObject<HTMLElement | null>, onClose: () => void, anchor: RefObject<HTMLElement | null> | undefined) {
  const close = useRef(onClose);
  close.current = onClose;
  // 그래프(React Flow)와 터미널은 mousedown 전파를 막는다. 그래서 pointerdown을 캡처 단계에서 받는다.
  useEffect(() => {
    const outside = (e: PointerEvent) => {
      const target = e.target as Node;
      if (!box.current?.contains(target) && !anchor?.current?.contains(target)) close.current();
    };
    document.addEventListener('pointerdown', outside, true);
    return () => document.removeEventListener('pointerdown', outside, true);
  }, []);
}
