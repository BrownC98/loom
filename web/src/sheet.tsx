import { Check, X } from 'lucide-react';
import { useEffect, useRef, useState, type MouseEvent as ReactMouseEvent, type PointerEvent as ReactPointerEvent, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useEscapeClose } from './popover';
import { LONG_PRESS_MS, SLOP_PX } from './touchTerminal';

const PHONE_QUERY = '(max-width: 639px) and (pointer: coarse)';

export function usePhone(): boolean {
  const [phone, setPhone] = useState(() => matchMedia(PHONE_QUERY).matches);
  useEffect(() => {
    const media = matchMedia(PHONE_QUERY);
    const change = () => setPhone(media.matches);
    media.addEventListener('change', change);
    return () => media.removeEventListener('change', change);
  }, []);
  return phone;
}

const DRAG_CLOSE_PX = 80;

type SheetEntry = { loomSheet: number };

export function isSheetEntry(state: unknown): state is SheetEntry {
  return typeof state === 'object' && state !== null && typeof (state as SheetEntry).loomSheet === 'number';
}

let openSheets = 0;

function useSheetHistory(onClose: () => void) {
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    const depth = ++openSheets;
    history.pushState({ loomSheet: depth } satisfies SheetEntry, '');
    const onPop = () => {
      const now = isSheetEntry(history.state) ? history.state.loomSheet : 0;
      if (now < depth) close.current();
    };
    window.addEventListener('popstate', onPop);
    return () => {
      window.removeEventListener('popstate', onPop);
      openSheets--;
      if (isSheetEntry(history.state) && history.state.loomSheet === depth) history.back();
    };
  }, []);
}

export function Sheet(props: { label: string; onClose: () => void; children: ReactNode }) {
  const { t } = useTranslation();
  const sheet = useEscapeClose<HTMLDivElement>(props.onClose);
  const dragFrom = useRef<number | null>(null);
  useSheetHistory(props.onClose);
  const drag = (e: ReactPointerEvent) => {
    if (dragFrom.current === null || !sheet.current) return;
    sheet.current.style.transform = `translateY(${Math.max(0, e.clientY - dragFrom.current)}px)`;
  };
  const drop = (e: ReactPointerEvent) => {
    if (dragFrom.current === null || !sheet.current) return;
    const dy = e.clientY - dragFrom.current;
    dragFrom.current = null;
    sheet.current.style.transform = '';
    if (dy > DRAG_CLOSE_PX) props.onClose();
  };
  return (
    <div className="phone-scrim" onClick={(e) => e.target === e.currentTarget && props.onClose()}>
      <div ref={sheet} className="phone-sheet" role="dialog" aria-modal="true" aria-label={props.label}>
        <div
          className="sheet-head"
          onPointerDown={(e) => {
            if ((e.target as HTMLElement).closest('button')) return;
            dragFrom.current = e.clientY;
            e.currentTarget.setPointerCapture(e.pointerId);
          }}
          onPointerMove={drag}
          onPointerUp={drop}
          onPointerCancel={drop}
        >
          <span className="grip" aria-hidden="true" />
          <h3>{props.label}</h3>
          <button type="button" className="sheet-close" aria-label={t('phone.closeSheet')} onClick={props.onClose}>
            <X aria-hidden="true" />
          </button>
        </div>
        <div className="sheet-body">{props.children}</div>
      </div>
    </div>
  );
}

export type MenuItem = { label: string; icon?: ReactNode; run: () => unknown; checked?: boolean; danger?: boolean; confirm?: boolean; disabled?: boolean };

export function MenuSheet(props: { title: string; items: MenuItem[]; onClose: () => void }) {
  const { t } = useTranslation();
  const [armed, setArmed] = useState<string | null>(null);
  const [refusal, setRefusal] = useState<string | null>(null);
  const press = (item: MenuItem) => {
    if (item.confirm && armed !== item.label) return setArmed(item.label);
    setRefusal(null);
    Promise.resolve(item.run()).then(props.onClose, (err: Error) => setRefusal(err.message));
  };
  return (
    <Sheet label={props.title} onClose={props.onClose}>
      {props.items.map((item) => (
        <button key={item.label} type="button" className={item.danger ? 'btn danger' : 'btn'} disabled={item.disabled} onClick={() => press(item)}>
          {item.icon}
          {armed === item.label ? t('phone.removeConfirm') : item.label}
          {item.checked && <Check className="mark" aria-hidden="true" />}
        </button>
      ))}
      <button type="button" className="btn ghost" onClick={props.onClose}>
        {t('phone.cancel')}
      </button>
      {refusal && (
        <p className="refusal" role="alert">
          {refusal}
        </p>
      )}
    </Sheet>
  );
}

export function useLongPress(onLong: (row: HTMLElement) => void) {
  const timer = useRef<number | undefined>(undefined);
  const start = useRef<{ x: number; y: number } | null>(null);
  const cancel = () => {
    window.clearTimeout(timer.current);
    start.current = null;
  };
  useEffect(() => cancel, []);
  return {
    onPointerDown: (e: ReactPointerEvent<HTMLElement>) => {
      if (e.pointerType === 'mouse') return;
      start.current = { x: e.clientX, y: e.clientY };
      const row = e.currentTarget;
      timer.current = window.setTimeout(() => {
        start.current = null;
        swallowNextClick();
        onLong(row);
      }, LONG_PRESS_MS);
    },
    onPointerMove: (e: ReactPointerEvent<HTMLElement>) => {
      if (start.current && Math.hypot(e.clientX - start.current.x, e.clientY - start.current.y) > SLOP_PX) cancel();
    },
    onPointerUp: cancel,
    onPointerCancel: cancel,
    onContextMenu: (e: ReactMouseEvent) => e.preventDefault(),
  };
}

function swallowNextClick() {
  const swallow = (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    window.removeEventListener('pointerdown', drop, true);
  };
  const drop = () => window.removeEventListener('click', swallow, true);
  window.addEventListener('click', swallow, { capture: true, once: true });
  window.addEventListener('pointerdown', drop, { capture: true, once: true });
}
