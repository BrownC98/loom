// 버튼이 여는 메뉴. 「정렬」 방향 메뉴와 노드 메뉴(⋯)가 쓴다(목업 docs/mockups/relaunch.html ③).
import { useEffect, useState, type KeyboardEvent, type ReactNode, type RefObject } from 'react';
import { createPortal } from 'react-dom';
import { useDismiss } from './popover';

/** 메뉴 폭. styles.css `.menu`의 폭이다. 메뉴 오른쪽을 버튼 오른쪽에 맞춘다. */
const MENU_WIDTH = 180;

/**
 * `anchor` 버튼 아래에 연 메뉴. 그래프 층에 덮이지 않게 body에 그린다. 메뉴와 버튼 밖을 누르면 닫는다(그래프를 옮기기 시작해도).
 * 위아래 화살표는 켜진 항목 사이를 돈다. Esc는 닫고 버튼으로 돌아간다(`useDismiss`). `autoFocus` 항목이 없으면 첫 켜진 항목에 선다.
 */
export function Menu(props: { anchor: RefObject<HTMLElement | null>; label: string; onClose: () => void; children: ReactNode }) {
  const box = useDismiss<HTMLDivElement>(props.onClose, { anchor: props.anchor });
  const [at] = useState(() => {
    const r = props.anchor.current?.getBoundingClientRect();
    return r ? { top: r.bottom + 4, left: r.right - MENU_WIDTH } : { top: 0, left: 0 };
  });
  const live = () => [...(box.current?.querySelectorAll<HTMLButtonElement>('button:enabled') ?? [])];
  useEffect(() => {
    if (!box.current?.contains(document.activeElement)) live()[0]?.focus();
  }, []);
  const onKey = (e: KeyboardEvent) => {
    const items = live();
    const now = items.indexOf(document.activeElement as HTMLButtonElement);
    const step = e.key === 'ArrowDown' ? 1 : e.key === 'ArrowUp' ? -1 : 0;
    if (step) {
      e.preventDefault();
      items[(now + step + items.length) % items.length]?.focus();
    }
  };
  return createPortal(
    <div ref={box} className="menu" role="menu" aria-label={props.label} style={at} onKeyDown={onKey}>
      {props.children}
    </div>,
    document.body,
  );
}
