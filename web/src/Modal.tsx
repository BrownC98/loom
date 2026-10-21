import { useEffect, useRef, type ReactNode } from 'react';

/** 화면 최상층에 띄운 창(`<dialog>`). Esc와 포커스 가두기는 브라우저가 한다. `outsideClose`면 바깥(배경) 누르기도 `onClose`다.
 *  그때 내용은 안쪽 상자가 감싸야 한다. 창 자체의 여백을 눌러도 바깥으로 본다. */
export function Modal(props: { className: string; label: string; onClose: () => void; outsideClose?: boolean; children: ReactNode }) {
  const ref = useRef<HTMLDialogElement>(null);
  const onClose = useRef(props.onClose);
  onClose.current = props.onClose;
  useEffect(() => {
    const dialog = ref.current!;
    dialog.showModal();
    const close = (e: Event) => {
      e.preventDefault();
      onClose.current();
    };
    dialog.addEventListener('cancel', close);
    return () => dialog.removeEventListener('cancel', close);
  }, []);
  return (
    <dialog ref={ref} className={props.className} aria-label={props.label} onClick={props.outsideClose ? (e) => e.target === e.currentTarget && props.onClose() : undefined}>
      {props.children}
    </dialog>
  );
}
