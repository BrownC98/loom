import { ArrowUp, Image as ImageIcon, ImagePlus, X } from 'lucide-react';
import { useEffect, useLayoutEffect, useRef, useState, type DOMAttributes, type MouseEvent, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { answerWait, chooseOption, dismissWait, doneImageUrl, PASTE_LIMIT_BYTES, typeText } from './api';
import { ImageList, MB, useImages, ZoomImage } from './attachments';
import { DocBlocks } from './DocText';
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
import { bySince, closesInBody, kindIcon, NoticeBadges, oldestWait, screenOf, showsId } from './notice';
import { NoticeActions, pressScreen } from './noticeActions';
import { KeyHint, PromptBody } from './PromptBody';

export function waitingNodes(nodes: ReadonlyMap<NodeId, NodeView>): NodeView[] {
  return [...nodes.values()].filter((v) => v.waits.length > 0).sort((a, b) => bySince(oldestWait(a), oldestWait(b)));
}

export const waitCount = (waiting: NodeView[]) => waiting.reduce((n, v) => n + v.waits.length, 0);

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
            <b data-find>{v.name}</b>
            <NoticeBadges waits={v.waits} />
          </button>
          <ul className="wait-rows">
            {[...v.waits].sort(bySince).map((w) => (
              <WaitRow key={w.id} node={v.id} wait={w} onFocus={() => props.onFocus(v.id)} />
            ))}
          </ul>
        </li>
      ))}
    </ul>
  );
}

export const entryClick = (run: () => void) => (e: MouseEvent<HTMLElement>) => {
  if (!(e.target instanceof Element) || e.target.closest('button, input, textarea, a, label')) return;
  if (window.getSelection()?.toString()) return;
  run();
};

function WaitRow(props: { node: NodeId; wait: ScreenWait; onFocus: () => void }) {
  return (
    <li
      className="wait-row"
      onClick={entryClick(props.onFocus)}
    >
      <WaitEntry node={props.node} wait={props.wait} />
    </li>
  );
}

function WaitEntry(props: { node: NodeId; wait: ScreenWait }) {
  const { wait } = props;
  return (
    <div className="entry" data-tone={wait.tone}>
      <WaitHead wait={wait} />
      {wait.instruction && <Instruction text={wait.instruction} />}
      <WaitBody node={props.node} wait={wait} />
    </div>
  );
}

function WaitTitle(props: { wait: ScreenWait }) {
  const { t } = useTranslation();
  const { wait } = props;
  const Icon = kindIcon(wait);
  return (
    <>
      <Icon className="kind-mark" aria-hidden="true" />
      <span className="kind">{t(`wait.${wait.kind}`)}</span>
      {showsId(wait.kind) && (
        <span className="qid" data-find-id={wait.id}>
          Q{wait.id}
        </span>
      )}
    </>
  );
}

export function WaitHead(props: { wait: ScreenWait; children?: ReactNode }) {
  const { wait } = props;
  return (
    <div className="top">
      <WaitTitle wait={wait} />
      <Published wait={wait} />
      {props.children}
      {wait.tone === 'done' ? <ConfirmButton wait={wait.id} /> : !closesInBody(wait.kind) && <DismissButton wait={wait.id} />}
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
  const [before, after] = t('inbox.instruction', { text: '\u0000' }).split('\u0000');
  return (
    <p className="instruction">
      {before}
      <span data-find>{props.text}</span>
      {after}
    </p>
  );
}

export function ConfirmButton(props: { wait: WaitId; onClose?: () => () => void }) {
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
        const cancel = props.onClose?.();
        dismissWait(props.wait).catch((err: Error) => {
          cancel?.();
          setRefusal(err.message);
        });
      }}
    >
      {t('inbox.confirm')}
    </button>
  );
}

export function WaitContent(props: { node: NodeId; wait: ScreenWait; hints?: boolean; onClose?: () => () => void }) {
  const { wait } = props;
  const { prompt } = wait;
  const [refusal, setRefusal] = useState<string | null>(null);
  const [choosing, setChoosing] = useState(false);
  const answerPrompt = (send: () => Promise<null>) => {
    setRefusal(null);
    setChoosing(true);
    const cancel = props.onClose?.();
    const sent = send();
    sent
      .catch((err: Error) => {
        cancel?.();
        setRefusal(err.message);
      })
      .finally(() => setChoosing(false));
    return sent;
  };
  return (
    <>
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
          hints={props.hints}
          onPick={(option) => answerPrompt(() => chooseOption(wait.id, prompt, option))}
          onType={(text, option) => answerPrompt(() => typeText(wait.id, prompt, text, option))}
        />
      )}
      {!prompt && (wait.text || wait.doc) && (
        <div className="full" data-find>
          <DocBlocks blocks={wait.blocks ?? []} doc={wait.doc} node={props.node} onRefusal={setRefusal} />
        </div>
      )}
      {refusal && (
        <p className="refusal" role="alert">
          {refusal}
        </p>
      )}
    </>
  );
}

function WaitBody(props: { node: NodeId; wait: ScreenWait }) {
  const { t } = useTranslation();
  const { wait } = props;
  const done = wait.tone === 'done';
  const qid = showsId(wait.kind) ? `Q${wait.id}` : t(`wait.${wait.kind}`);
  const screen = screenOf(wait);
  const [refusal, setRefusal] = useState<string | null>(null);
  const images = useImages(`wait:${wait.id}`, props.node, setRefusal);
  const [dragging, setDragging] = useState(false);
  const tell = (sent: Promise<unknown>) => {
    sent.catch((err: Error) => setRefusal(err.message));
    return sent.then(() => null);
  };
  const send = (text: string) => {
    setRefusal(null);
    return tell(images.paths().then((saved) => answerWait(wait.id, text, saved.paths).then(() => images.forget(saved.ids))));
  };
  const typeOn = (rows: string[]) => (text: string) => {
    setRefusal(null);
    return tell(pressScreen(wait.id, rows, { kind: 'text', value: text }));
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
      <WaitContent node={props.node} wait={wait} />
      {wait.answerable && (
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
      )}
      <NoticeActions node={props.node} wait={wait} />
      {screen && <AnswerForm wait={wait.id} qid={qid} placeholder={t('inbox.screenPlaceholder')} hasImages={false} onSend={typeOn(screen)} />}
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

export function Choices(props: { list: readonly Choice[]; onPick: (key: string) => void; hints?: boolean }) {
  const { t } = useTranslation();
  const { list } = props;
  if (list.length === 0) return null;
  return (
    <div className="choices">
      {list.map((c, i) => (
        <button key={c.key} type="button" className="choice" data-recommended={c.recommended || undefined} onClick={() => props.onPick(c.key)}>
          {props.hints && <KeyHint index={i} />}
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

function fitAnswer(el: HTMLTextAreaElement) {
  el.style.height = 'auto';
  if (el.offsetParent === null) return;
  el.style.height = `${el.scrollHeight + el.offsetHeight - el.clientHeight}px`;
}

function AnswerForm(props: { wait: WaitId; qid: string; placeholder: string; hasImages: boolean; onSend: (text: string) => Promise<null>; onPasteImages?: (files: File[]) => void }) {
  const { t } = useTranslation();
  const [text, setText] = useDraft(String(props.wait));
  const send = () => props.onSend(text).then(() => setText(''), () => {});
  const box = useRef<HTMLTextAreaElement>(null);
  const ready = text.trim() !== '' || props.hasImages;
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    const watch = new ResizeObserver(() => fitAnswer(el));
    watch.observe(el);
    return () => watch.disconnect();
  }, []);
  useLayoutEffect(() => {
    if (box.current) fitAnswer(box.current);
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
          props.onPasteImages?.(files);
        }}
        aria-label={t('inbox.answerLabel', { id: props.qid })}
        placeholder={props.placeholder}
      />
      {props.onPasteImages && (
        <label className="attach" title={t('inbox.attachImage')}>
          <ImageIcon aria-hidden="true" />
          <input
            type="file"
            accept="image/*"
            multiple
            aria-label={t('inbox.attachImage')}
            onChange={(e) => {
              props.onPasteImages?.([...(e.target.files ?? [])]);
              e.target.value = '';
            }}
          />
        </label>
      )}
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

export function OsNotifier(props: { waiting: NodeView[]; enabled: boolean; onSelect: (id: NodeId) => void }) {
  const { t } = useTranslation();
  const show = useShowNodes();
  const seen = useRef<Set<WaitId> | null>(null);
  const select = useRef(props.onSelect);
  select.current = (id) => {
    show([id]);
    props.onSelect(id);
  };
  useEffect(() => {
    const keys = new Set(props.waiting.flatMap((v) => v.waits.map((w) => w.id)));
    const before = seen.current;
    seen.current = keys;
    if (before === null || !props.enabled || !notifyAllowed()) return;
    const fresh = props.waiting.flatMap((v) => v.waits.filter((w) => !before.has(w.id)).map((w) => ({ v, w })));
    if (fresh.length === 0) return;
    windowHidden().then((hidden) => {
      if (!hidden) return;
      for (const { v, w } of fresh) notify(`${v.name} · ${t(`wait.${w.kind}`)}`, w.summary, `loom-wait-${w.id}`, () => select.current(v.id));
    });
  }, [props.waiting, props.enabled]);
  return null;
}
