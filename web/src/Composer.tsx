import { ArrowUp, ChevronDown, Image as ImageIcon, Paperclip, RotateCcw, SquareTerminal } from 'lucide-react';
import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore, type ClipboardEvent, type ComponentProps, type CSSProperties, type DOMAttributes, type DragEvent, type KeyboardEvent, type PointerEvent as ReactPointerEvent, type ReactNode, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import { savePastedFile } from './api';
import { droppedFiles, ImageList, isImage, quotePath, tooBig, useImages } from './attachments';
import { IME_KEY_CODE, watchComposition, type Composition } from './composition';
import { useShowNodes } from './camera';
import { editDraft, readDraft, useDraft } from './drafts';
import type { FloatPlace } from './gen/FloatPlace';
import type { FloatSize } from './gen/FloatSize';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { RepoView } from './gen/RepoView';
import { COMPOSER_HOME, FLOAT_MARGIN, floatZoom, placeOf, popSpace, useFloatDrag, useSize } from './floatPlace';
import { nodeInput, useNodeInput, type NodeInput } from './nodeInputs';
import { NodePick } from './NodePick';

export type ComposerKeys = { sends: (e: KeyboardEvent<HTMLTextAreaElement>) => boolean; escape: (() => void) | null; blank: ((input: NodeInput) => void) | null };

export const deskKeys = (escape: () => void): ComposerKeys => ({
  sends: (e) => e.key === 'Enter' && !e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey && !e.nativeEvent.isComposing && e.keyCode !== IME_KEY_CODE,
  escape,
  blank: null,
});

export const PHONE_KEYS: ComposerKeys = { sends: () => false, escape: null, blank: (input) => input.enter() };

const COMPOSER_ROOM = '--composer-room';
const PICK_WIDTH = 380;

type ComposerProps = { node: NodeId; name: string; keys: ComposerKeys; onTyped?: (before: string, after: string) => boolean };
type FloatHead = { onDragStart: (e: ReactPointerEvent<HTMLElement>) => void; onReset: (() => void) | null; onName: () => void; list: { button: RefObject<HTMLButtonElement | null>; open: boolean; onToggle: () => void } };

let focusOnMount: NodeId | null = null;

function keyed<T>(none: T) {
  const values = new Map<string, T>();
  const heard = new Set<() => void>();
  const listen = (heed: () => void) => {
    heard.add(heed);
    return () => void heard.delete(heed);
  };
  const get = (key: string) => values.get(key) ?? none;
  const set = (key: string, next: T) => {
    if (next === none) values.delete(key);
    else values.set(key, next);
    heard.forEach((heed) => heed());
  };
  return { get, set, use: (key: string) => useSyncExternalStore(listen, () => get(key)) };
}

const NO_UPLOADS: readonly Promise<unknown>[] = [];
const uploads = keyed(NO_UPLOADS);
const sendings = keyed(false);
const refusals = keyed<string | null>(null);

function holdUpload(key: string, upload: Promise<unknown>) {
  uploads.set(key, [...uploads.get(key), upload]);
  void upload.finally(() => {
    const rest = uploads.get(key).filter((u) => u !== upload);
    uploads.set(key, rest.length > 0 ? rest : NO_UPLOADS);
  });
}

export function CanvasComposer(props: ComposerProps & { place: FloatPlace | null; size: FloatSize; onPlace: (place: FloatPlace | null) => void; views: ReadonlyMap<NodeId, NodeView>; repos: readonly RepoView[]; onPick: (id: NodeId) => void }) {
  const input = useNodeInput(props.node);
  const layer = useRef<HTMLDivElement>(null);
  const float = useRef<HTMLDivElement>(null);
  const listButton = useRef<HTMLButtonElement>(null);
  const [listing, setListing] = useState(false);
  const pane = useSize(layer);
  const box = useSize(float);
  const drag = useFloatDrag(float, pane, box, props.onPlace);
  const show = useShowNodes();
  const home = props.place === null;
  const focusField = () => float.current?.querySelector('textarea')?.focus();
  const pick = (id: NodeId) => {
    setListing(false);
    show([id]);
    if (id === props.node) return focusField();
    focusOnMount = id;
    props.onPick(id);
  };
  useLayoutEffect(() => {
    const canvas = layer.current?.parentElement;
    if (!canvas || !box || !home) return;
    canvas.style.setProperty(COMPOSER_ROOM, `${box.h + 14}px`);
    return () => void canvas.style.removeProperty(COMPOSER_ROOM);
  }, [box?.h, home]);
  if (!input) return null;
  const at = drag.at ?? (pane && box ? placeOf(props.place ?? COMPOSER_HOME, pane, box) : null);
  const zoom = { '--float-zoom': floatZoom(props.size) } as CSSProperties;
  const space = at && box && pane ? popSpace(at.top, box, pane) : null;
  const pickWidth = pane ? Math.min(PICK_WIDTH * floatZoom(props.size), pane.w - 2 * FLOAT_MARGIN) : 0;
  const pickLeft = at && pane ? Math.max(FLOAT_MARGIN, Math.min(at.left, pane.w - FLOAT_MARGIN - pickWidth)) - at.left : 0;
  return (
    <div ref={layer} className="composer-layer">
      <div ref={float} className="composer-float" data-dragging={drag.at ? true : undefined} style={at ? { ...zoom, left: at.left, top: at.top } : { ...zoom, visibility: 'hidden' }} onClickCapture={drag.swallowClick}>
        <Composer {...props} float={{ onDragStart: drag.start, onReset: home ? null : () => props.onPlace(null), onName: () => show([props.node]), list: { button: listButton, open: listing, onToggle: () => setListing(!listing) } }} />
        {listing && space && (
          <div className="node-pick-at" data-above={space.above || undefined} style={{ left: pickLeft }}>
            <NodePick
              views={props.views}
              repos={props.repos}
              current={props.node}
              anchor={listButton}
              width={pickWidth / floatZoom(props.size)}
              room={space.room / floatZoom(props.size)}
              onPick={pick}
              onClose={() => setListing(false)}
              onEscape={() => {
                setListing(false);
                focusField();
              }}
            />
          </div>
        )}
      </div>
    </div>
  );
}

export function Composer(props: ComposerProps & { float?: FloatHead }) {
  const { t } = useTranslation();
  const key = `composer:${props.node}`;
  const [text, setText] = useDraft(key);
  const refusal = refusals.use(key);
  const setRefusal = (message: string | null) => refusals.set(key, message);
  const [dragging, setDragging] = useState(false);
  const images = useImages(key, props.node, setRefusal);
  const area = useRef<HTMLTextAreaElement>(null);
  const composition = useRef<Composition | null>(null);
  useEffect(() => {
    const wanted = focusOnMount === props.node;
    focusOnMount = null;
    if (wanted) requestAnimationFrame(() => area.current?.focus());
  }, []);
  useEffect(() => {
    if (!area.current) return;
    const watched = watchComposition(area.current);
    composition.current = watched;
    return () => {
      watched.dispose();
      composition.current = null;
    };
  }, []);
  useLayoutEffect(() => {
    const el = area.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${el.scrollHeight + el.offsetHeight - el.clientHeight}px`;
  }, [text]);
  const filing = uploads.use(key).length;
  const sending = sendings.use(key);
  const ready = text.trim() !== '' || images.list.length > 0 || filing > 0 || props.keys.blank !== null;

  const insertPath = (path: string) => {
    const el = area.current;
    let caret: number | null = null;
    editDraft(key, (now) => {
      const at = el?.isConnected && el.value === now ? el.selectionStart : now.length;
      const typed = `${at > 0 && !/\s/.test(now[at - 1]) ? ' ' : ''}${quotePath(path)} `;
      caret = at + typed.length;
      return now.slice(0, at) + typed + now.slice(at);
    });
    requestAnimationFrame(() => caret !== null && el?.isConnected && el.setSelectionRange(caret, caret));
  };
  const take = (files: File[]) => {
    if (sendings.get(key)) return;
    setRefusal(null);
    images.attach(files.filter(isImage));
    for (const file of files.filter((f) => !isImage(f))) {
      const big = tooBig(file);
      if (big) setRefusal(big);
      else holdUpload(key, savePastedFile(file, props.node).then(insertPath, (err: Error) => setRefusal(err.message)));
    }
  };
  const deliverable = (): NodeInput => {
    const input = nodeInput(props.node);
    const state = input?.state() ?? 'gone';
    if (!input || state !== 'open') throw new Error(state === 'offline' ? t('composer.offline') : t('composer.gone'));
    return input;
  };

  const send = () => {
    setRefusal(null);
    if (sendings.get(key)) return;
    sendings.set(key, true);
    const attached = images.list.length;
    Promise.all([images.paths(), Promise.all(uploads.get(key))])
      .then(([saved]) => {
        const body = readDraft(key).trimEnd();
        const input = deliverable();
        if (body.trim() === '' && saved.paths.length === 0) {
          if (attached > 0) throw new Error(t('composer.nothingSaved'));
          props.keys.blank?.(input);
          return;
        }
        input.paste([body, saved.paths.map(quotePath).join(' ')].filter((part) => part !== '').join('\n'));
        input.enter();
        editDraft(key, (now) => (now.trimEnd() === body ? '' : now));
        images.forget(saved.ids);
      })
      .catch((err: Error) => setRefusal(err.message))
      .finally(() => sendings.set(key, false));
  };
  const submit = () => {
    if (!composition.current?.afterText(send)) send();
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Escape' && props.keys.escape) {
      e.preventDefault();
      props.keys.escape();
    } else if (props.keys.sends(e)) {
      e.preventDefault();
      if (ready && !sending) submit();
    }
  };
  const onPaste = (e: ClipboardEvent<HTMLTextAreaElement>) => {
    const files = [...e.clipboardData.files];
    if (files.length === 0) return;
    e.preventDefault();
    take(files);
  };
  const hasFiles = (e: DragEvent) => e.dataTransfer.types.includes('Files');
  const onDrop = (e: DragEvent) => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    setDragging(false);
    const { files, errors } = droppedFiles(e.dataTransfer);
    take(files);
    if (errors.length > 0) setRefusal(errors.join('\n'));
  };
  return (
    <ComposerFace
      name={props.name}
      float={props.float}
      dragging={dragging}
      drop={{
        onDragOver: (e) => {
          if (!hasFiles(e)) return;
          e.preventDefault();
          e.dataTransfer.dropEffect = 'copy';
          setDragging(true);
        },
        onDragLeave: (e) => {
          if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDragging(false);
        },
        onDrop,
      }}
      field={{
        ref: area,
        value: text,
        readOnly: sending,
        onChange: (e) => {
          if (!props.onTyped?.(text, e.target.value)) setText(e.target.value);
        },
        onKeyDown,
        onPaste,
      }}
      sendable={ready && !sending}
      onFiles={take}
      onSubmit={submit}
    >
      <ImageList images={images.list} onRemove={images.remove} />
      {filing > 0 && <p className="filing">{t('composer.filing', { count: filing })}</p>}
      {refusal && (
        <p className="refusal" role="alert">
          {refusal}
        </p>
      )}
    </ComposerFace>
  );
}

export function ComposerFace(props: {
  name: string;
  float?: FloatHead;
  dragging: boolean;
  drop: Pick<DOMAttributes<HTMLDivElement>, 'onDragOver' | 'onDragLeave' | 'onDrop'>;
  field: Pick<ComponentProps<'textarea'>, 'ref' | 'value' | 'readOnly' | 'onChange' | 'onKeyDown' | 'onPaste'>;
  sendable: boolean;
  onFiles: (files: File[]) => void;
  onSubmit: () => void;
  children?: ReactNode;
}) {
  const { t } = useTranslation();
  const picker = (accept: string | undefined, label: string, Icon: typeof ImageIcon, className: string) => (
    <label className={`attach ${className}`} title={label}>
      <Icon aria-hidden="true" />
      <input
        type="file"
        accept={accept}
        multiple
        aria-label={label}
        onChange={(e) => {
          props.onFiles([...(e.target.files ?? [])]);
          e.target.value = '';
        }}
      />
    </label>
  );
  return (
    <div className="composer" data-dragging={props.dragging || undefined} {...props.drop}>
      <div className="composer-to" onPointerDown={props.float?.onDragStart}>
        <span className="target">
          <SquareTerminal aria-hidden="true" />
          {props.float ? (
            <>
              <button type="button" className="node-name" title={t('composer.showNode')} onClick={props.float.onName}>
                <b>{props.name}</b>
              </button>
              <button ref={props.float.list.button} type="button" className="node-list" aria-label={t('composer.pickNode')} title={t('composer.pickNode')} aria-haspopup="dialog" aria-expanded={props.float.list.open} onClick={props.float.list.onToggle}>
                <ChevronDown aria-hidden="true" />
              </button>
            </>
          ) : (
            <b>{props.name}</b>
          )}
          {t('composer.to')}
        </span>
        <span className="keys">{t('composer.keys')}</span>
        {props.float?.onReset && (
          <button type="button" className="float-reset" aria-label={t('composer.resetPlace')} title={t('composer.resetPlace')} onClick={props.float.onReset}>
            <RotateCcw aria-hidden="true" />
          </button>
        )}
      </div>
      {props.children}
      <form
        className="composer-line"
        onSubmit={(e) => {
          e.preventDefault();
          if (props.sendable) props.onSubmit();
        }}
      >
        <textarea
          {...props.field}
          rows={1}
          aria-label={t('composer.label', { name: props.name })}
          placeholder={t('composer.placeholder')}
          autoComplete="off"
          autoCapitalize="off"
          spellCheck={false}
        />
        {picker('image/*', t('composer.attachImage'), ImageIcon, 'image')}
        {picker(undefined, t('composer.attachFile'), Paperclip, 'file')}
        <button type="submit" className="send" aria-label={t('composer.send')} title={t('composer.send')} disabled={!props.sendable}>
          <ArrowUp aria-hidden="true" />
        </button>
      </form>
    </div>
  );
}
