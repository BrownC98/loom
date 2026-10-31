import { Maximize2, Minus, Plus } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { attachPinchZoom, containInBox, type Point, type Size, type View, zoomAbout } from './pinchZoom';

const FIT: View = { x: 0, y: 0, zoom: 1 };
const LIMITS = { min: 1, max: 6 };
const TAP_SLOP = 8;
const FIT_SHARE = 0.92;
const BUTTON_STEP = 1.25;
const WHEEL_RATE = 0.002;
const WHEEL_LINE_PX = 16;

type Fit = Size & { scale: number };

function fitOf(image: HTMLImageElement, box: Size): Fit {
  const scale = Math.min(1, (box.width * FIT_SHARE) / image.naturalWidth, (box.height * FIT_SHARE) / image.naturalHeight);
  return { width: image.naturalWidth * scale, height: image.naturalHeight * scale, scale };
}

export function PinchImage(props: { src: string; alt: string; onClose: () => void }) {
  const { t } = useTranslation();
  const root = useRef<HTMLDivElement>(null);
  const image = useRef<HTMLImageElement>(null);
  const fit = useRef<Fit | null>(null);
  const view = useRef<View>(FIT);
  const [shown, setShown] = useState<{ fit: Fit; view: View } | null>(null);
  const drag = useRef<{ x: number; y: number; from: View } | null>(null);
  const moved = useRef(false);

  const box = () => root.current!.getBoundingClientRect();
  const set = (v: View) => {
    if (!fit.current) return;
    const next = containInBox(v, fit.current, box());
    view.current = next;
    setShown({ fit: fit.current, view: next });
  };
  const measure = () => {
    if (!image.current?.naturalWidth) return;
    fit.current = fitOf(image.current, box());
    set(FIT);
  };
  const zoomAt = (zoom: number, at: Point) => set(zoomAbout(view.current, zoom, at, LIMITS));
  const step = (factor: number) => {
    const actual = 1 / fit.current!.scale;
    const now = view.current.zoom;
    const next = now * factor;
    const { width, height } = box();
    zoomAt((now < actual && next > actual) || (now > actual && next < actual) ? actual : next, { x: width / 2, y: height / 2 });
  };
  const start = (x: number, y: number) => {
    moved.current = false;
    drag.current = { x, y, from: view.current };
  };
  const move = (x: number, y: number) => {
    if (!drag.current) return;
    const dx = x - drag.current.x;
    const dy = y - drag.current.y;
    if (Math.hypot(dx, dy) > TAP_SLOP) moved.current = true;
    if (view.current.zoom > 1) set({ ...drag.current.from, x: drag.current.from.x + dx, y: drag.current.from.y + dy });
  };

  useEffect(() => {
    const el = root.current!;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const delta = e.deltaMode === WheelEvent.DOM_DELTA_LINE ? e.deltaY * WHEEL_LINE_PX : e.deltaY;
      const r = el.getBoundingClientRect();
      zoomAt(view.current.zoom * Math.exp(-delta * WHEEL_RATE), { x: e.clientX - r.left, y: e.clientY - r.top });
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    const sizes = new ResizeObserver(measure);
    sizes.observe(el);
    const pinch = attachPinchZoom(el, { get: () => view.current, set }, LIMITS);
    return () => {
      el.removeEventListener('wheel', onWheel);
      sizes.disconnect();
      pinch();
    };
  }, []);

  const zoom = shown?.view.zoom ?? 1;
  return (
    <>
      <div
        ref={root}
        className="pinch-image"
        data-zoomed={zoom > 1 || undefined}
        onPointerDown={(e) => e.pointerType === 'mouse' && e.button === 0 && start(e.clientX, e.clientY)}
        onPointerMove={(e) => {
          if (e.pointerType !== 'mouse') return;
          if (e.buttons & 1) move(e.clientX, e.clientY);
          else drag.current = null;
        }}
        onPointerUp={(e) => e.pointerType === 'mouse' && (drag.current = null)}
        onTouchStart={(e) => {
          const t = e.touches[0];
          if (e.touches.length === 1) start(t.clientX, t.clientY);
          else drag.current = null;
        }}
        onTouchMove={(e) => e.touches.length === 1 && move(e.touches[0].clientX, e.touches[0].clientY)}
        onTouchEnd={(e) => {
          if (e.touches.length > 0) moved.current = true;
          drag.current = null;
        }}
        onClick={(e) => {
          e.stopPropagation();
          if (moved.current) return;
          if (e.target === image.current && view.current.zoom > 1) set(FIT);
          else props.onClose();
        }}
      >
        <img
          ref={image}
          src={props.src}
          alt={props.alt}
          draggable={false}
          onLoad={measure}
          style={
            shown
              ? { width: shown.fit.width, height: shown.fit.height, transform: `translate(${shown.view.x}px, ${shown.view.y}px) scale(${zoom})` }
              : { visibility: 'hidden' }
          }
        />
      </div>
      {shown && (
        <div className="image-zoom-bar">
          <button type="button" aria-label={t('inbox.zoomOut')} title={t('inbox.zoomOut')} disabled={zoom <= LIMITS.min} onClick={() => step(1 / BUTTON_STEP)}>
            <Minus aria-hidden="true" />
          </button>
          <output>{Math.round(shown.fit.scale * zoom * 100)}%</output>
          <button type="button" aria-label={t('inbox.zoomIn')} title={t('inbox.zoomIn')} disabled={zoom >= LIMITS.max} onClick={() => step(BUTTON_STEP)}>
            <Plus aria-hidden="true" />
          </button>
          <span className="sep" />
          <button type="button" onClick={() => set(FIT)}>
            <Maximize2 aria-hidden="true" />
            {t('inbox.fitImage')}
          </button>
        </div>
      )}
    </>
  );
}
