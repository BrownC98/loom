// 팝오버를 밖 누르기로 닫는다. Esc는 팝오버의 onKeyDown이 닫는다.
import { useEffect, useRef } from 'react';

/** 돌려준 ref를 단 요소 밖을 누르면 `onClose`를 부른다. */
export function useOutsideClose<T extends HTMLElement>(onClose: () => void) {
  const box = useRef<T>(null);
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    const outside = (e: PointerEvent) => {
      if (!box.current?.contains(e.target as Node)) close.current();
    };
    document.addEventListener('pointerdown', outside, true);
    return () => document.removeEventListener('pointerdown', outside, true);
  }, []);
  return box;
}
