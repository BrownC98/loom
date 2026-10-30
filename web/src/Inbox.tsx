import { ArrowRight, ArrowUp, ChevronDown, Image as ImageIcon, ImagePlus, RotateCcw, X } from 'lucide-react';
import { useEffect, useLayoutEffect, useRef, useState, type DOMAttributes, type PointerEvent as ReactPointerEvent, type ReactNode, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import type { FloatPlace } from './gen/FloatPlace';
import type { FloatSize } from './gen/FloatSize';
import type { QuestionPill } from './gen/QuestionPill';
import { FLOAT_MARGIN, floatZoom, PILL_HOME, placeOf, popLeft, popSpace, useFloatDrag, useSize, type Size } from './floatPlace';
import { answerWait, chooseOption, dismissWait, doneImageUrl, PASTE_LIMIT_BYTES, typeText } from './api';
import { ImageList, MB, useImages, ZoomImage } from './attachments';
import { DocText } from './DocText';
import { dateAndTime } from './i18n';
import { notify, notifyAllowed, windowHidden } from './osNotify';
import type { Choice } from './gen/Choice';
import type { ScreenWait } from './gen/ScreenWait';
import type { NodeId } from './gen/NodeId';
import type { WaitId } from './gen/WaitId';
import type { NodeView } from './gen/NodeView';
import { useShowNodes } from './camera';
import { useDraft } from './drafts';
import { stateClass } from './NodeCard';
import { bySince, ICON, newestId, NodeText, NoticeBadges, NoticeLead, oldestWait, pillClosed, showsId } from './notice';
import { PromptBody } from './PromptBody';
import { useDismiss } from './popover';

export function waitingNodes(nodes: ReadonlyMap<NodeId, NodeView>): NodeView[] {
  return [...nodes.values()].filter((v) => v.waits.length > 0).sort((a, b) => bySince(oldestWait(a), oldestWait(b)));
}

export const waitCount = (waiting: NodeView[]) => waiting.reduce((n, v) => n + v.waits.length, 0);

export function Inbox(props: { waiting: NodeView[]; notify: boolean; pill: QuestionPill; place: FloatPlace | null; size: FloatSize; onPlace: (place: FloatPlace | null) => void; closed: WaitId | null; onClose: (newest: WaitId) => void; onSelect: (id: NodeId) => void; onFocus: (id: NodeId) => void }) {
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
  const group = useRef<HTMLDivElement>(null);
  const box = useSize(pill);
  const drag = useFloatDrag(group, pane, box, props.onPlace);

  useEffect(() => {
    if (n === 0 || !shown) setOpen(false);
  }, [n, shown]);

  const at = drag.at ?? (pane && box ? placeOf(props.place ?? PILL_HOME, pane, box) : null);
  return (
    <div className="inbox" ref={paneRef}>
      {first && shown && (
        <div ref={group} className="inbox-group" style={at ? { left: at.left, top: at.top } : { visibility: 'hidden' }}>
          <div ref={pill} className="inbox-pill" style={{ zoom: floatZoom(props.size) }} data-view={props.pill} data-tone={firstWait.tone} data-dragging={drag.at ? true : undefined} onClickCapture={drag.swallowClick}>
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
const popWidth = (pane: Size) => Math.min(POP_WIDTH, Math.max(0, pane.w - 2 * FLOAT_MARGIN));

function InboxPop(props: { label: string; pill: RefObject<HTMLDivElement | null>; left: number; width: number; above: boolean; room: number; onClose: () => void; children: ReactNode }) {
  const box = useDismiss<HTMLDivElement>(props.onClose, { anchor: props.pill });
  return (
    <div ref={box} className="pop inbox-pop" data-above={props.above || undefined} style={{ left: props.left, width: props.width, maxHeight: props.room }} role="dialog" aria-label={props.label}>
      <h2>{props.label}</h2>
      {props.children}
    </div>
  );
}

function PillHead(props: { node: NodeView; wait: ScreenWait; waits: readonly ScreenWait[]; line: boolean; open: boolean; onToggle: () => void; onDragStart: (e: ReactPointerEvent<HTMLElement>) => void; onReset: (() => void) | null; onClose: () => void }) {
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
        <button type="button" className="float-reset" aria-label={t('inbox.resetPlace')} title={t('inbox.resetPlace')} onClick={props.onReset}>
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
        <li key={v.id} className="wait-node" data-tone={oldestWait(v).tone}>
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

function WaitRow(props: { node: NodeId; wait: ScreenWait; onGo: () => void; onFocus: () => void }) {
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

function WaitEntry(props: { node: NodeId; wait: ScreenWait; onGo: () => void }) {
  const { t } = useTranslation();
  const { wait } = props;
  const Icon = ICON[wait.tone];
  return (
    <div className="entry" data-tone={wait.tone}>
      <div className="top">
        <Icon className="kind-mark" aria-hidden="true" />
        <span className="kind">{t(`wait.${wait.kind}`)}</span>
        {showsId(wait.kind) && <span className="qid">Q{wait.id}</span>}
        <Published wait={wait} />
        {wait.tone === 'done' ? <ConfirmButton wait={wait.id} /> : <DismissButton wait={wait.id} />}
      </div>
      {wait.instruction && <Instruction text={wait.instruction} />}
      <WaitBody node={props.node} wait={wait} onGo={props.onGo} />
    </div>
  );
}

export function Published(props: { wait: ScreenWait }) {
  const { t } = useTranslation();
  const at = props.wait.since * 1000;
  return <time className="published" dateTime={new Date(at).toISOString()}>{t('inbox.published', { time: dateAndTime(at) })}</time>;
}

export function Instruction(props: { text: string }) {
  const { t } = useTranslation();
  return <p className="instruction">{t('inbox.instruction', { text: props.text })}</p>;
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

function WaitBody(props: { node: NodeId; wait: ScreenWait; onGo: () => void }) {
  const { t } = useTranslation();
  const { wait } = props;
  const { prompt } = wait;
  const done = wait.tone === 'done';
  const qid = showsId(wait.kind) ? `Q${wait.id}` : t(`wait.${wait.kind}`);
  const [refusal, setRefusal] = useState<string | null>(null);
  const [choosing, setChoosing] = useState(false);
  const images = useImages(`wait:${wait.id}`, props.node, setRefusal);
  const [dragging, setDragging] = useState(false);
  const send = (text: string) => {
    setRefusal(null);
    const sent = images.paths().then((saved) => answerWait(wait.id, text, saved.paths).then(() => images.forget(saved.ids)));
    sent.catch((err: Error) => setRefusal(err.message));
    return sent.then(() => null);
  };
  const answerPrompt = (sent: Promise<null>) => {
    setRefusal(null);
    setChoosing(true);
    sent.catch((err: Error) => setRefusal(err.message)).finally(() => setChoosing(false));
    return sent;
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
    <div className="wait-body" data-tone={wait.tone} data-dragging={dragging || undefined} {...drop}>
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
          draft={String(wait.id)}
          busy={choosing}
          onPick={(option) => answerPrompt(chooseOption(wait.id, prompt, option))}
          onType={(text, option) => answerPrompt(typeText(wait.id, prompt, text, option))}
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
          <ImageList images={images.list} onRemove={images.remove} />
          <AnswerForm
            wait={wait.id}
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

function AnswerForm(props: { wait: WaitId; qid: string; placeholder: string; hasImages: boolean; onSend: (text: string) => Promise<null>; onPasteImages: (files: File[]) => void }) {
  const { t } = useTranslation();
  const [text, setText] = useDraft(String(props.wait));
  const send = () => props.onSend(text).then(() => setText(''), () => {});
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
        if (ready) send();
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
          if (ready) send();
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
      for (const { v, w } of fresh) notify(`${v.name} · ${t(`wait.${w.kind}`)}`, w.summary, `loom-wait-${w.id}`, () => select.current(v.id));
    });
  }, [waiting, enabled]);
}
