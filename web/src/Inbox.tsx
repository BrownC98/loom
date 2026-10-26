import { ArrowRight, ArrowUp, ChevronDown, Image as ImageIcon, ImagePlus, RotateCcw, X } from 'lucide-react';
import { useEffect, useLayoutEffect, useRef, useState, type DOMAttributes, type PointerEvent as ReactPointerEvent, type ReactNode, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import type { PillPlace } from './gen/PillPlace';
import type { QuestionPill } from './gen/QuestionPill';
import { DRAG_SLOP, PILL_MARGIN, placeFrom, placeOf, popLeft, popSpace, type Size } from './pillPlace';
import { answerWait, chooseOption, dismissWait, doneImageUrl, PASTE_LIMIT_BYTES, savePastedFile, typeText } from './api';
import { DocText } from './DocText';
import { dateAndTime } from './i18n';
import { notify, notifyAllowed, windowHidden } from './osNotify';
import type { Choice } from './gen/Choice';
import type { HumanWait } from './gen/HumanWait';
import type { NodeId } from './gen/NodeId';
import type { WaitId } from './gen/WaitId';
import type { NodeView } from './gen/NodeView';
import { useShowNodes } from './camera';
import { stateClass } from './NodeCard';
import { bySince, ICON, newestId, NodeText, NoticeBadges, NoticeLead, oldestWait, pillClosed, showsId, summaryOf, toneOf } from './notice';
import { PinchImage } from './PinchImage';
import { PromptBody } from './PromptBody';
import { Modal } from './Modal';
import { useDismiss } from './popover';

export function waitingNodes(nodes: ReadonlyMap<NodeId, NodeView>): NodeView[] {
  return [...nodes.values()].filter((v) => v.waits.length > 0).sort((a, b) => bySince(oldestWait(a), oldestWait(b)));
}

export const waitCount = (waiting: NodeView[]) => waiting.reduce((n, v) => n + v.waits.length, 0);

export function Inbox(props: { waiting: NodeView[]; notify: boolean; pill: QuestionPill; place: PillPlace | null; onPlace: (place: PillPlace | null) => void; closed: WaitId | null; onClose: (newest: WaitId) => void; onSelect: (id: NodeId) => void; onFocus: (id: NodeId) => void }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const show = useShowNodes();
  const go = (id: NodeId) => {
    show([id]);
    props.onSelect(id);
  };
  useOsNotify(props.waiting, props.notify, go);
  const pill = useRef<HTMLDivElement>(null);
  const waits = props.waiting.flatMap((v) => v.waits);
  const n = waits.length;
  const shown = props.pill !== 'off' && !pillClosed(props.closed, waits);
  const first = props.waiting[0];
  const firstWait = first && oldestWait(first);
  const line = props.pill === 'line';
  const toList = () => setOpen(!open);
  const paneRef = useRef<HTMLDivElement>(null);
  const pane = useSize(paneRef);
  const box = useSize(pill);
  const drag = usePillDrag(pill, pane, box, props.onPlace);

  useEffect(() => {
    if (n === 0 || !shown) setOpen(false);
  }, [n, shown]);

  const at = drag.at ?? (pane && box ? placeOf(props.place, pane, box) : null);
  return (
    <div className="inbox" ref={paneRef}>
      {first && shown && (
        <div className="inbox-group" style={at ? { left: at.left, top: at.top } : { visibility: 'hidden' }}>
          <div ref={pill} className="inbox-pill" data-view={props.pill} data-tone={toneOf(firstWait.kind)} data-dragging={drag.at ? true : undefined} onClickCapture={drag.swallowClick}>
            <PillHead node={first} wait={firstWait} waits={waits} line={line} open={open} onToggle={toList} onDragStart={drag.start} onReset={props.place ? () => props.onPlace(null) : null} onClose={() => props.onClose(newestId(waits))} />
            {!line && (
              <div className="pill-row">
                <WaitEntry key={firstWait.id} node={first.id} wait={firstWait} onGo={() => go(first.id)} />
              </div>
            )}
          </div>
          {open && n > 0 && at && pane && box && (
            <InboxPop label={t('inbox.title', { count: n })} pill={pill} left={popLeft(at.left, box.w, popWidth(pane), pane.w)} width={popWidth(pane)} {...popSpace(at.top, box, pane)} onClose={() => setOpen(false)}>
              <WaitList
                waiting={props.waiting}
                onGo={go}
                onFocus={(id) => {
                  show([id]);
                  props.onFocus(id);
                }}
              />
            </InboxPop>
          )}
        </div>
      )}
    </div>
  );
}

const POP_WIDTH = 520;
const popWidth = (pane: Size) => Math.min(POP_WIDTH, Math.max(0, pane.w - 2 * PILL_MARGIN));

function useSize(ref: RefObject<HTMLElement | null>): Size | null {
  const [size, setSize] = useState<Size | null>(null);
  const watched = useRef<{ el: HTMLElement; watch: ResizeObserver } | null>(null);
  useLayoutEffect(() => {
    const el = ref.current;
    if (watched.current?.el === el) return;
    watched.current?.watch.disconnect();
    watched.current = null;
    if (!el) {
      setSize(null);
      return;
    }
    const read = () => setSize((was) => (was && was.w === el.offsetWidth && was.h === el.offsetHeight ? was : { w: el.offsetWidth, h: el.offsetHeight }));
    read();
    const watch = new ResizeObserver(read);
    watch.observe(el);
    watched.current = { el, watch };
  });
  useEffect(() => () => watched.current?.watch.disconnect(), []);
  return size;
}

function usePillDrag(pill: RefObject<HTMLElement | null>, pane: Size | null, box: Size | null, onPlace: (place: PillPlace) => void) {
  const [at, setAt] = useState<{ left: number; top: number } | null>(null);
  const moved = useRef(false);
  const start = (e: ReactPointerEvent<HTMLElement>) => {
    if (e.button !== 0 || !pane || !box || !(e.target instanceof Element) || e.target.closest('input, textarea, a, label, .pill-reset, .pill-close')) return;
    const group = pill.current?.parentElement;
    if (!group) return;
    const from = { x: e.clientX, y: e.clientY, left: group.offsetLeft, top: group.offsetTop };
    const dragged = (ev: PointerEvent) => ({ left: from.left + ev.clientX - from.x, top: from.top + ev.clientY - from.y, far: Math.hypot(ev.clientX - from.x, ev.clientY - from.y) > DRAG_SLOP });
    const place = (ev: PointerEvent) => {
      const d = dragged(ev);
      return placeFrom(d.left, d.top, pane, box);
    };
    const move = (ev: PointerEvent) => {
      const d = dragged(ev);
      if (!d.far && !moved.current) return;
      moved.current = true;
      setAt(placeOf(place(ev), pane, box));
    };
    const end = (ev: PointerEvent) => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', end);
      window.removeEventListener('pointercancel', end);
      if (moved.current) {
        onPlace(place(ev));
        setAt(null);
        setTimeout(() => (moved.current = false));
      }
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', end);
    window.addEventListener('pointercancel', end);
  };
  const swallowClick = (e: { stopPropagation: () => void; preventDefault: () => void }) => {
    if (!moved.current) return;
    e.stopPropagation();
    e.preventDefault();
  };
  return { at, start, swallowClick };
}

function InboxPop(props: { label: string; pill: RefObject<HTMLDivElement | null>; left: number; width: number; above: boolean; room: number; onClose: () => void; children: ReactNode }) {
  const box = useDismiss<HTMLDivElement>(props.onClose, { anchor: props.pill });
  return (
    <div ref={box} className="pop inbox-pop" data-above={props.above || undefined} style={{ left: props.left, width: props.width, maxHeight: props.room }} role="dialog" aria-label={props.label}>
      <h2>{props.label}</h2>
      {props.children}
    </div>
  );
}

function PillHead(props: { node: NodeView; wait: HumanWait; waits: readonly HumanWait[]; line: boolean; open: boolean; onToggle: () => void; onDragStart: (e: ReactPointerEvent<HTMLElement>) => void; onReset: (() => void) | null; onClose: () => void }) {
  const { t } = useTranslation();
  const { wait, open } = props;
  return (
    <div className="head" aria-live="polite" onPointerDown={props.onDragStart}>
      <button type="button" className="pill-close" aria-label={t('inbox.closePill')} title={t('inbox.closePill')} onClick={props.onClose}>
        <X aria-hidden="true" />
      </button>
      <NoticeLead wait={wait} waits={props.waits} />
      <button type="button" className="go" aria-expanded={open} aria-label={t('inbox.listLabel', { count: props.waits.length })} onClick={props.onToggle}>
        {props.line ? <NodeText name={props.node.name} wait={wait} /> : <b>{props.node.name}</b>}
        {!props.line && <ChevronDown className={open ? 'chev open' : 'chev'} aria-hidden="true" />}
      </button>
      {props.onReset && (
        <button type="button" className="pill-reset" aria-label={t('inbox.resetPlace')} title={t('inbox.resetPlace')} onClick={props.onReset}>
          <RotateCcw aria-hidden="true" />
        </button>
      )}
    </div>
  );
}

export function InboxPanel(props: { waiting: NodeView[]; onSelect: (id: NodeId) => void; onFocus: (id: NodeId) => void }) {
  const { t } = useTranslation();
  const show = useShowNodes();
  if (props.waiting.length === 0) return <p className="inbox-empty">{t('inbox.empty')}</p>;
  return (
    <WaitList
      waiting={props.waiting}
      onGo={(id) => {
        show([id]);
        props.onSelect(id);
      }}
      onFocus={(id) => {
        show([id]);
        props.onFocus(id);
      }}
    />
  );
}

function WaitList(props: { waiting: NodeView[]; onGo: (id: NodeId) => void; onFocus: (id: NodeId) => void }) {
  const { t } = useTranslation();
  return (
    <ul className="wait-list">
      {props.waiting.map((v) => (
        <li key={v.id} className="wait-node" data-tone={toneOf(oldestWait(v).kind)}>
          <button type="button" className="go" aria-label={t('inbox.jumpLabel', { name: v.name })} onClick={() => props.onGo(v.id)}>
            <span className={`dot ${stateClass(v)}`} aria-hidden="true" />
            <b>{v.name}</b>
            <NoticeBadges waits={v.waits} />
          </button>
          <ul className="wait-rows">
            {[...v.waits].sort(bySince).map((w) => (
              <WaitRow key={w.id} node={v.id} wait={w} onGo={() => props.onGo(v.id)} onFocus={() => props.onFocus(v.id)} />
            ))}
          </ul>
        </li>
      ))}
    </ul>
  );
}

function WaitRow(props: { node: NodeId; wait: HumanWait; onGo: () => void; onFocus: () => void }) {
  return (
    <li
      className="wait-row"
      onClick={(e) => {
        if (!(e.target instanceof Element) || e.target.closest('button, input, textarea, a, label')) return;
        if (window.getSelection()?.toString()) return;
        props.onFocus();
      }}
    >
      <WaitEntry node={props.node} wait={props.wait} onGo={props.onGo} />
    </li>
  );
}

function WaitEntry(props: { node: NodeId; wait: HumanWait; onGo: () => void }) {
  const { t } = useTranslation();
  const { wait } = props;
  const tone = toneOf(wait.kind);
  const Icon = ICON[tone];
  return (
    <div className="entry" data-tone={tone}>
      <div className="top">
        <Icon className="kind-mark" aria-hidden="true" />
        <span className="kind">{t(`wait.${wait.kind}`)}</span>
        {showsId(wait.kind) && <span className="qid">Q{wait.id}</span>}
        <Published wait={wait} />
        {tone === 'done' ? <ConfirmButton wait={wait.id} /> : <DismissButton wait={wait.id} />}
      </div>
      {wait.instruction && <Instruction text={wait.instruction} />}
      <WaitBody node={props.node} wait={wait} onGo={props.onGo} />
    </div>
  );
}

function Published(props: { wait: HumanWait }) {
  const { t } = useTranslation();
  const at = props.wait.since * 1000;
  return <time className="published" dateTime={new Date(at).toISOString()}>{t('inbox.published', { time: dateAndTime(at) })}</time>;
}

function Instruction(props: { text: string }) {
  const { t } = useTranslation();
  return <p className="instruction">{t('inbox.instruction', { text: props.text })}</p>;
}

function ZoomImage(props: { src: string; name: string; title: string }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return (
    <>
      <button type="button" className="zoom" title={props.title} aria-label={t('inbox.zoomImage', { name: props.name })} onClick={() => setOpen(true)}>
        <img src={props.src} alt={props.name} />
      </button>
      {open && (
        <Modal className="image-view" label={props.name} onClose={() => setOpen(false)} outsideClose>
          <PinchImage src={props.src} alt={props.name} onTap={() => setOpen(false)} />
        </Modal>
      )}
    </>
  );
}

function ConfirmButton(props: { wait: WaitId }) {
  const { t } = useTranslation();
  const [refusal, setRefusal] = useState<string | null>(null);
  return (
    <button
      type="button"
      className="choice confirm"
      title={refusal ?? undefined}
      data-refused={refusal !== null || undefined}
      onClick={() => {
        setRefusal(null);
        dismissWait(props.wait).catch((err: Error) => setRefusal(err.message));
      }}
    >
      {t('inbox.confirm')}
    </button>
  );
}

function WaitBody(props: { node: NodeId; wait: HumanWait; onGo: () => void }) {
  const { t } = useTranslation();
  const { wait } = props;
  const { prompt } = wait;
  const done = toneOf(wait.kind) === 'done';
  const qid = showsId(wait.kind) ? `Q${wait.id}` : t(`wait.${wait.kind}`);
  const [refusal, setRefusal] = useState<string | null>(null);
  const [choosing, setChoosing] = useState(false);
  const images = useImages(props.node, setRefusal);
  const [dragging, setDragging] = useState(false);
  const send = (text: string) => {
    setRefusal(null);
    answerWait(
      wait.id,
      text,
      images.list.map((i) => i.path),
    ).catch((err: Error) => setRefusal(err.message));
  };
  const answerPrompt = (sent: Promise<null>) => {
    setRefusal(null);
    setChoosing(true);
    sent.catch((err: Error) => setRefusal(err.message)).finally(() => setChoosing(false));
  };
  const drop: DOMAttributes<HTMLDivElement> = wait.answerable
    ? {
        onDragOver: (e) => {
          if (!e.dataTransfer.types.includes('Files')) return;
          e.preventDefault();
          e.dataTransfer.dropEffect = 'copy';
          setDragging(true);
        },
        onDragLeave: (e) => {
          if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDragging(false);
        },
        onDrop: (e) => {
          if (!e.dataTransfer.types.includes('Files')) return;
          e.preventDefault();
          setDragging(false);
          images.attach([...e.dataTransfer.files]);
        },
      }
    : {};
  return (
    <div className="wait-body" data-tone={toneOf(wait.kind)} data-dragging={dragging || undefined} {...drop}>
      {wait.images.length > 0 && (
        <ul className="answer-images">
          {wait.images.map((path, index) => (
            <li key={path}>
              <ZoomImage src={doneImageUrl(wait.id, index)} name={path.split(/[\\/]/).pop() ?? path} title={path} />
            </li>
          ))}
        </ul>
      )}
      {prompt && (
        <PromptBody
          prompt={prompt}
          busy={choosing}
          onPick={(option) => answerPrompt(chooseOption(wait.id, prompt, option))}
          onType={(text) => answerPrompt(typeText(wait.id, prompt, text))}
        />
      )}
      {!prompt && (wait.text || wait.doc) && (
        <p className="full">
          <DocText parts={wait.parts ?? []} doc={wait.doc} node={props.node} onRefusal={setRefusal} />
        </p>
      )}
      {wait.answerable ? (
        <>
          {!done && <Choices list={wait.choices ?? []} onPick={send} />}
          {images.list.length > 0 && (
            <ul className="answer-images">
              {images.list.map((image) => (
                <li key={image.path}>
                  <ZoomImage src={image.url} name={image.name} title={image.name} />
                  <button type="button" aria-label={t('inbox.removeImage')} title={t('inbox.removeImage')} onClick={() => images.remove(image)}>
                    <X aria-hidden="true" />
                  </button>
                </li>
              ))}
            </ul>
          )}
          <AnswerForm
            qid={qid}
            placeholder={t(done ? 'inbox.followUpPlaceholder' : 'inbox.answerPlaceholder')}
            hasImages={images.list.length > 0}
            onSend={send}
            onPasteImages={images.attach}
          />
        </>
      ) : (
        <button type="button" className="to-terminal" onClick={props.onGo}>
          {t('inbox.toTerminal')}
          <ArrowRight aria-hidden="true" />
        </button>
      )}
      {refusal && (
        <p className="refusal" role="alert">
          {refusal}
        </p>
      )}
      {dragging && (
        <div className="drop-cover" aria-hidden="true">
          <ImagePlus />
          {t('inbox.dropImage', { id: qid })}
          <span className="sub">{t('inbox.dropLimit', { limit: PASTE_LIMIT_BYTES / MB })}</span>
        </div>
      )}
    </div>
  );
}

const MB = 1024 * 1024;

type AttachedImage = { path: string; url: string; name: string };

function useImages(node: NodeId, onError: (message: string) => void) {
  const { t } = useTranslation();
  const [list, setList] = useState<AttachedImage[]>([]);
  const shown = useRef<AttachedImage[]>([]);
  shown.current = list;
  useEffect(() => () => shown.current.forEach((i) => URL.revokeObjectURL(i.url)), []);
  const attach = (files: File[]) => {
    for (const file of files) {
      if (!file.type.startsWith('image/')) {
        onError(t('inbox.notImage', { name: file.name }));
        continue;
      }
      if (file.size > PASTE_LIMIT_BYTES) {
        onError(t('terminal.tooBig', { name: file.name, size: Math.ceil(file.size / MB), limit: PASTE_LIMIT_BYTES / MB }));
        continue;
      }
      savePastedFile(file, node).then(
        (path) => setList((now) => [...now, { path, url: URL.createObjectURL(file), name: file.name }]),
        (err: Error) => onError(err.message),
      );
    }
  };
  const remove = (image: AttachedImage) => {
    URL.revokeObjectURL(image.url);
    setList((now) => now.filter((i) => i !== image));
  };
  return { list, attach, remove };
}

function Choices(props: { list: readonly Choice[]; onPick: (key: string) => void }) {
  const { t } = useTranslation();
  const { list } = props;
  if (list.length === 0) return null;
  return (
    <div className="choices">
      {list.map((c) => (
        <button key={c.key} type="button" className="choice" data-recommended={c.recommended || undefined} onClick={() => props.onPick(c.key)}>
          <span className="key">{c.key}</span>
          <span className="label">
            {c.label}
            {c.recommended && <span className="mark">{t('inbox.recommended')}</span>}
          </span>
        </button>
      ))}
    </div>
  );
}

function AnswerForm(props: { qid: string; placeholder: string; hasImages: boolean; onSend: (text: string) => void; onPasteImages: (files: File[]) => void }) {
  const { t } = useTranslation();
  const [text, setText] = useState('');
  const box = useRef<HTMLTextAreaElement>(null);
  const ready = text.trim() !== '' || props.hasImages;
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    const fit = () => {
      if (el.offsetParent === null) return;
      el.style.height = 'auto';
      el.style.height = `${el.scrollHeight + el.offsetHeight - el.clientHeight}px`;
    };
    fit();
    const watch = new ResizeObserver(fit);
    watch.observe(el);
    return () => watch.disconnect();
  }, [text]);
  return (
    <form
      className="answer"
      onSubmit={(e) => {
        e.preventDefault();
        if (ready) props.onSend(text);
      }}
    >
      <textarea
        ref={box}
        rows={1}
        value={text}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key !== 'Enter' || e.shiftKey || e.nativeEvent.isComposing) return;
          e.preventDefault();
          if (ready) props.onSend(text);
        }}
        onPaste={(e) => {
          const files = [...e.clipboardData.files].filter((f) => f.type.startsWith('image/'));
          if (files.length === 0) return;
          e.preventDefault();
          props.onPasteImages(files);
        }}
        aria-label={t('inbox.answerLabel', { id: props.qid })}
        placeholder={props.placeholder}
      />
      <label className="attach" title={t('inbox.attachImage')}>
        <ImageIcon aria-hidden="true" />
        <input
          type="file"
          accept="image/*"
          multiple
          aria-label={t('inbox.attachImage')}
          onChange={(e) => {
            props.onPasteImages([...(e.target.files ?? [])]);
            e.target.value = '';
          }}
        />
      </label>
      <button type="submit" className="send" aria-label={t('inbox.send', { id: props.qid })} disabled={!ready}>
        <ArrowUp aria-hidden="true" />
      </button>
    </form>
  );
}

function DismissButton(props: { wait: WaitId }) {
  const { t } = useTranslation();
  const [refusal, setRefusal] = useState<string | null>(null);
  return (
    <button
      type="button"
      className="dismiss"
      aria-label={t('inbox.dismiss')}
      title={refusal ?? t('inbox.dismiss')}
      data-refused={refusal !== null || undefined}
      onClick={() => {
        setRefusal(null);
        dismissWait(props.wait).catch((err: Error) => setRefusal(err.message));
      }}
    >
      <X aria-hidden="true" />
    </button>
  );
}

function useOsNotify(waiting: NodeView[], enabled: boolean, onSelect: (id: NodeId) => void) {
  const { t } = useTranslation();
  const seen = useRef<Set<WaitId> | null>(null);
  const select = useRef(onSelect);
  select.current = onSelect;
  useEffect(() => {
    const keys = new Set(waiting.flatMap((v) => v.waits.map((w) => w.id)));
    const before = seen.current;
    seen.current = keys;
    if (before === null || !enabled || !notifyAllowed()) return;
    const fresh = waiting.flatMap((v) => v.waits.filter((w) => !before.has(w.id)).map((w) => ({ v, w })));
    if (fresh.length === 0) return;
    windowHidden().then((hidden) => {
      if (!hidden) return;
      for (const { v, w } of fresh) notify(`${v.name} · ${t(`wait.${w.kind}`)}`, summaryOf(w), `loom-wait-${w.id}`, () => select.current(v.id));
    });
  }, [waiting, enabled]);
}
