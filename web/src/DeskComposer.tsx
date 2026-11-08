import { ArrowUp, ChevronDown, RotateCcw, SquareTerminal, type LucideIcon } from 'lucide-react';
import { useEffect, useLayoutEffect, useRef, useState, type ComponentProps, type CSSProperties, type DOMAttributes, type KeyboardEvent, type PointerEvent as ReactPointerEvent, type ReactNode, type Ref } from 'react';
import { useTranslation } from 'react-i18next';
import { answerWait } from './api';
import { AlertCard } from './AlertCard';
import { stepKey, type Route, type Target, type TargetKey } from './alertTarget';
import { useShowNodes } from './camera';
import { answerWith, ComposerNotes, FilePick, typeInto, useComposer, type ComposerKeys, type Delivery } from './Composer';
import { COMPOSER_HOME, FLOAT_MARGIN, floatZoom, placeOf, popSpace, useFloatDrag, useSize } from './floatPlace';
import type { FloatPlace } from './gen/FloatPlace';
import type { FloatSize } from './gen/FloatSize';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import { useNodeInput } from './nodeInputs';
import { ICON } from './notice';
import { TargetList } from './TargetList';

const COMPOSER_ROOM = '--composer-room';
const COMPOSER_WIDTH = 680;

const ROUTE_ICON: Record<Route['kind'], LucideIcon> = { answer: ICON.ask, continue: ICON.done, prompt: SquareTerminal };
const PLACEHOLDER = { answer: 'inbox.answerPlaceholder', continue: 'inbox.followUpPlaceholder', prompt: 'composer.placeholder' } as const satisfies Record<Route['kind'], string>;

let focusOnMount: NodeId | null = null;

type DeskProps = {
  target: Target;
  waiting: NodeView[];
  keys: ComposerKeys;
  place: FloatPlace | null;
  size: FloatSize;
  corner: number;
  onPlace: (place: FloatPlace | null) => void;
  onTarget: (node: NodeId, key: TargetKey) => void;
  onReaim: () => void;
  onSelect: (id: NodeId) => void;
  onTerminal: () => void;
};

export function DeskComposer(props: DeskProps) {
  const { t } = useTranslation();
  const { target } = props;
  const node = target.view.id;
  const input = useNodeInput(node);
  const layer = useRef<HTMLDivElement>(null);
  const float = useRef<HTMLDivElement>(null);
  const targetButton = useRef<HTMLButtonElement>(null);
  const [listing, setListing] = useState(false);
  const pane = useSize(layer);
  const box = useSize(float);
  const fullWidth = pane ? Math.min(COMPOSER_WIDTH * floatZoom(props.size), pane.w - 2 * FLOAT_MARGIN) : null;
  const fullBox = box && fullWidth !== null ? { w: fullWidth, h: box.h } : null;
  const drag = useFloatDrag(float, pane, fullBox, props.onPlace);
  const show = useShowNodes();
  const home = props.place === null;
  const deliver: Delivery = (text, paths) => {
    if (target.lost) {
      props.onReaim();
      throw new Error(t('composer.targetClosed'));
    }
    return target.route.kind === 'prompt' ? typeInto(node)(text, paths) : answerWith(target.route.wait.id)(text, paths);
  };
  const composer = useComposer({ node, keys: props.keys, deliver });
  const focusField = () => float.current?.querySelector('textarea')?.focus();
  const closeList = () => {
    setListing(false);
    focusField();
  };
  const toggleList = () => (listing ? closeList() : setListing(true));
  const pick = (id: NodeId, key: TargetKey) => {
    setListing(false);
    show([id]);
    props.onTarget(id, key);
    props.onSelect(id);
    if (id === node) focusField();
    else focusOnMount = id;
  };
  useEffect(() => {
    const wanted = focusOnMount === node;
    focusOnMount = null;
    if (wanted) requestAnimationFrame(focusField);
  }, []);
  useLayoutEffect(() => {
    const canvas = layer.current?.parentElement;
    if (!canvas || !box || !home) return;
    canvas.style.setProperty(COMPOSER_ROOM, `${box.h + 14}px`);
    return () => void canvas.style.removeProperty(COMPOSER_ROOM);
  }, [box?.h, home]);
  const beside = home && !drag.at;
  const width = pane && fullWidth !== null && beside ? Math.max(0, Math.min(fullWidth, pane.w - props.corner - 2 * FLOAT_MARGIN)) : fullWidth;
  const placed = pane && box && width !== null ? placeOf(props.place ?? COMPOSER_HOME, pane, { w: width, h: box.h }) : null;
  const at = drag.at ?? (placed && pane && width !== null && beside ? { ...placed, left: Math.min(placed.left, pane.w - props.corner - FLOAT_MARGIN - width) } : placed);
  if (!input && target.total === 0) return null;
  const zoom = { '--float-zoom': floatZoom(props.size), width: width ?? undefined } as CSSProperties;
  const space = at && box && pane ? popSpace(at.top, box, pane) : null;
  const { route } = target;
  const choose = (key: string) => {
    const wait = target.card;
    if (!wait) return;
    composer.setRefusal(null);
    void composer.images
      .paths(node)
      .then((saved) => answerWait(wait.id, key, saved.paths).then(() => composer.images.forget(saved.ids)))
      .catch((err: Error) => composer.setRefusal(err.message));
  };
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!e.altKey || e.ctrlKey || e.metaKey || e.shiftKey || e.nativeEvent.isComposing) return;
    if (e.code === 'KeyL') toggleList();
    else if ((e.code === 'ArrowUp' || e.code === 'ArrowDown') && !listing) props.onTarget(node, stepKey(target, e.code === 'ArrowUp' ? -1 : 1));
    else return;
    e.preventDefault();
  };
  return (
    <div ref={layer} className="composer-layer">
      <div ref={float} className="composer-float" data-dragging={drag.at ? true : undefined} style={at ? { ...zoom, left: at.left, top: at.top } : { ...zoom, visibility: 'hidden' }} onClickCapture={drag.swallowClick} onKeyDown={onKeyDown}>
        <DeskFace
          route={route.kind}
          name={target.view.name}
          dragging={composer.dragging}
          drop={composer.drop}
          field={composer.field}
          sendable={composer.sendable}
          onFiles={composer.onFiles}
          onSubmit={composer.onSubmit}
          target={{ ref: targetButton, open: listing, onToggle: toggleList }}
          float={{ onDragStart: drag.start, onReset: home ? null : () => props.onPlace(null) }}
        >
          <AlertCard
            target={target}
            onChoose={choose}
            onGo={props.onTerminal}
            onFocus={() => {
              show([node]);
              props.onSelect(node);
            }}
          />
          <ComposerNotes composer={composer} />
          {target.lost && (
            <p className="refusal" role="alert">
              {t('composer.targetClosed')}
            </p>
          )}
        </DeskFace>
        {listing && space && (
          <div className="target-list-at" data-above={space.above || undefined}>
            <TargetList waiting={props.waiting} current={{ node, key: target.key }} currentView={target.view} anchor={targetButton} room={space.room / floatZoom(props.size)} onPick={pick} onClose={() => setListing(false)} onEscape={closeList} />
          </div>
        )}
      </div>
    </div>
  );
}

export function DeskFace(props: {
  route: Route['kind'];
  name: string;
  dragging: boolean;
  drop: Pick<DOMAttributes<HTMLDivElement>, 'onDragOver' | 'onDragLeave' | 'onDrop'>;
  field: Pick<ComponentProps<'textarea'>, 'ref' | 'value' | 'readOnly' | 'onChange' | 'onKeyDown' | 'onPaste'>;
  sendable: boolean;
  onFiles: (files: File[]) => void;
  onSubmit: () => void;
  target: { ref?: Ref<HTMLButtonElement>; open: boolean; onToggle: () => void };
  float?: { onDragStart: (e: ReactPointerEvent<HTMLElement>) => void; onReset: (() => void) | null };
  children?: ReactNode;
}) {
  const { t } = useTranslation();
  const Icon = ROUTE_ICON[props.route];
  return (
    <div className="composer desk" data-route={props.route} data-dragging={props.dragging || undefined} onPointerDown={props.float?.onDragStart} {...props.drop}>
      {props.children}
      <form
        className="composer-line"
        onSubmit={(e) => {
          e.preventDefault();
          if (props.sendable) props.onSubmit();
        }}
      >
        <button ref={props.target.ref} type="button" className="target-btn" title={t('composer.pickTarget')} aria-label={t('composer.pickTarget')} aria-haspopup="dialog" aria-expanded={props.target.open} onClick={props.target.onToggle}>
          <Icon aria-hidden="true" />
          <span>{t('composer.toNode', { name: props.name })}</span>
          <ChevronDown aria-hidden="true" />
        </button>
        <div className="composer-field">
          <FilePick onFiles={props.onFiles} />
          <textarea {...props.field} rows={1} aria-label={t('composer.label', { name: props.name })} placeholder={t(PLACEHOLDER[props.route])} autoComplete="off" autoCapitalize="off" spellCheck={false} />
          <span className="composer-keys">{t('composer.keys')}</span>
        </div>
        <button type="submit" className="send" aria-label={t('composer.send')} title={t('composer.send')} disabled={!props.sendable}>
          <ArrowUp aria-hidden="true" />
        </button>
        {props.float?.onReset && (
          <button type="button" className="float-reset" aria-label={t('composer.resetPlace')} title={t('composer.resetPlace')} onClick={props.float.onReset}>
            <RotateCcw aria-hidden="true" />
          </button>
        )}
      </form>
    </div>
  );
}
