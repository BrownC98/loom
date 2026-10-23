import { useEffect, useRef, type ReactNode } from 'react';

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
    <dialog
      ref={ref}
      className={props.className}
      aria-label={props.label}
      onKeyDown={(e) => e.key === 'Escape' && e.stopPropagation()}
      onClick={props.outsideClose ? (e) => e.target === e.currentTarget && props.onClose() : undefined}
    >
      {props.children}
    </dialog>
  );
}
