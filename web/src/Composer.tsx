import { ArrowUp, Paperclip } from 'lucide-react';
import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore, type ClipboardEvent, type ComponentProps, type DOMAttributes, type DragEvent, type KeyboardEvent, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { answerWait, savePastedFile } from './api';
import { droppedFiles, ImageList, isImage, quotePath, tooBig, useImages } from './attachments';
import { IME_KEY_CODE, watchComposition, type Composition } from './composition';
import { editDraft, readDraft, useDraft } from './drafts';
import type { NodeId } from './gen/NodeId';
import type { WaitId } from './gen/WaitId';
import i18next from './i18n';
import { nodeInput, type NodeInput } from './nodeInputs';

export type ComposerKeys = { sends: (e: KeyboardEvent<HTMLTextAreaElement>) => boolean; escape: (() => void) | null; blank: ((input: NodeInput) => void) | null };

export const deskKeys = (escape: () => void): ComposerKeys => ({
  sends: (e) => e.key === 'Enter' && !e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey && !e.nativeEvent.isComposing && e.keyCode !== IME_KEY_CODE,
  escape,
  blank: null,
});

export const PHONE_KEYS: ComposerKeys = { sends: () => false, escape: null, blank: (input) => input.enter() };

export type Delivery = (text: string, paths: readonly string[]) => void | Promise<void>;

function openInput(node: NodeId): NodeInput {
  const input = nodeInput(node);
  const state = input?.state() ?? 'gone';
  if (!input || state !== 'open') throw new Error(i18next.t(state === 'offline' ? 'composer.offline' : 'composer.gone'));
  return input;
}

export const typeInto =
  (node: NodeId): Delivery =>
  (text, paths) => {
    const input = openInput(node);
    input.paste([text, paths.map(quotePath).join(' ')].filter((part) => part !== '').join('\n'));
    input.enter();
  };

export const answerWith =
  (wait: WaitId): Delivery =>
  (text, paths) =>
    answerWait(wait, text, [...paths]).then(() => {});

type ComposerProps = { node: NodeId; name: string; keys: ComposerKeys; onTyped?: (before: string, after: string) => boolean };

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

const DRAFT_KEY = 'composer';

export const useDraftFilled = () => useDraft(DRAFT_KEY)[0].trim() !== '';

const NO_UPLOADS: readonly Promise<unknown>[] = [];
const uploads = keyed(NO_UPLOADS);
const sendings = keyed(false);
const refusals = keyed<string | null>(null);

type Filed = { node: NodeId; file: File; path: string };
const NO_FILED: readonly Filed[] = [];
const filed = keyed(NO_FILED);

function fileOn(file: File, node: NodeId) {
  return savePastedFile(file, node).then((path) => {
    filed.set(DRAFT_KEY, [...filed.get(DRAFT_KEY), { node, file, path }]);
    return path;
  });
}

function moveFiles(target: NodeId) {
  return Promise.allSettled(
    filed.get(DRAFT_KEY).map(async (f) => {
      const old = quotePath(f.path);
      if (f.node === target || !readDraft(DRAFT_KEY).includes(old)) return;
      const path = await savePastedFile(f.file, target);
      filed.set(DRAFT_KEY, filed.get(DRAFT_KEY).map((g) => (g === f ? { ...f, node: target, path } : g)));
      editDraft(DRAFT_KEY, (now) => now.replaceAll(old, quotePath(path)));
    }),
  ).then((all) => {
    const failed = all.find((r) => r.status === 'rejected');
    if (failed) throw failed.reason;
  });
}

function holdUpload(key: string, upload: Promise<unknown>) {
  uploads.set(key, [...uploads.get(key), upload]);
  void upload.finally(() => {
    const rest = uploads.get(key).filter((u) => u !== upload);
    uploads.set(key, rest.length > 0 ? rest : NO_UPLOADS);
  });
}

export function useComposer(props: { node: NodeId; keys: ComposerKeys; deliver: Delivery; onTyped?: (before: string, after: string) => boolean }) {
  const { t } = useTranslation();
  const [text, setText] = useDraft(DRAFT_KEY);
  const refusal = refusals.use(DRAFT_KEY);
  const setRefusal = (message: string | null) => refusals.set(DRAFT_KEY, message);
  const [dragging, setDragging] = useState(false);
  const images = useImages(DRAFT_KEY, props.node, setRefusal);
  const [area, setArea] = useState<HTMLTextAreaElement | null>(null);
  const composition = useRef<Composition | null>(null);
  useEffect(() => {
    if (!area) return;
    const watched = watchComposition(area);
    composition.current = watched;
    return () => {
      watched.dispose();
      composition.current = null;
    };
  }, [area]);
  useLayoutEffect(() => {
    const el = area;
    if (!el) return;
    const fit = () => {
      el.style.height = 'auto';
      if (el.value !== '') el.style.height = `${el.scrollHeight + el.offsetHeight - el.clientHeight}px`;
    };
    fit();
    let width = el.clientWidth;
    const resized = new ResizeObserver(() => {
      if (el.clientWidth === width) return;
      width = el.clientWidth;
      fit();
    });
    resized.observe(el);
    return () => resized.disconnect();
  }, [text, area]);
  const filing = uploads.use(DRAFT_KEY).length;
  const sending = sendings.use(DRAFT_KEY);
  const ready = text.trim() !== '' || images.list.length > 0 || filing > 0 || props.keys.blank !== null;

  const insertPath = (path: string) => {
    const el = area;
    let caret: number | null = null;
    editDraft(DRAFT_KEY, (now) => {
      const at = el?.isConnected && el.value === now ? el.selectionStart : now.length;
      const typed = `${at > 0 && !/\s/.test(now[at - 1]) ? ' ' : ''}${quotePath(path)} `;
      caret = at + typed.length;
      return now.slice(0, at) + typed + now.slice(at);
    });
    requestAnimationFrame(() => caret !== null && el?.isConnected && el.setSelectionRange(caret, caret));
  };
  const take = (files: File[]) => {
    if (sendings.get(DRAFT_KEY)) return;
    setRefusal(null);
    images.attach(files.filter(isImage));
    for (const file of files.filter((f) => !isImage(f))) {
      const big = tooBig(file);
      if (big) setRefusal(big);
      else holdUpload(DRAFT_KEY, fileOn(file, props.node).then(insertPath, (err: Error) => setRefusal(err.message)));
    }
  };

  const send = () => {
    setRefusal(null);
    if (sendings.get(DRAFT_KEY)) return;
    sendings.set(DRAFT_KEY, true);
    const attached = images.list.length;
    const target = props.node;
    const { deliver } = props;
    Promise.all([images.paths(target), Promise.all(uploads.get(DRAFT_KEY)).then(() => moveFiles(target))])
      .then(async ([saved]) => {
        const body = readDraft(DRAFT_KEY).trimEnd();
        if (body.trim() === '' && saved.paths.length === 0) {
          if (attached > 0) throw new Error(t('composer.nothingSaved'));
          props.keys.blank?.(openInput(target));
          return;
        }
        await deliver(body, saved.paths);
        editDraft(DRAFT_KEY, (now) => (now.trimEnd() === body ? '' : now));
        filed.set(DRAFT_KEY, filed.get(DRAFT_KEY).filter((f) => readDraft(DRAFT_KEY).includes(quotePath(f.path))));
        images.forget(saved.ids);
      })
      .catch((err: Error) => setRefusal(err.message))
      .finally(() => sendings.set(DRAFT_KEY, false));
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
  const drop: Pick<DOMAttributes<HTMLDivElement>, 'onDragOver' | 'onDragLeave' | 'onDrop'> = {
    onDragOver: (e) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = 'copy';
      setDragging(true);
    },
    onDragLeave: (e) => {
      if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDragging(false);
    },
    onDrop: (e) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      setDragging(false);
      const { files, errors } = droppedFiles(e.dataTransfer);
      take(files);
      if (errors.length > 0) setRefusal(errors.join('\n'));
    },
  };
  const field: Pick<ComponentProps<'textarea'>, 'ref' | 'value' | 'readOnly' | 'onChange' | 'onKeyDown' | 'onPaste'> = {
    ref: setArea,
    value: text,
    readOnly: sending,
    onChange: (e) => {
      if (!props.onTyped?.(text, e.target.value)) setText(e.target.value);
    },
    onKeyDown,
    onPaste,
  };
  return { field, drop, dragging, sendable: ready && !sending, onFiles: take, onSubmit: submit, images, refusal, filing, setRefusal };
}

export type ComposerState = ReturnType<typeof useComposer>;

export function ComposerNotes(props: { composer: ComposerState }) {
  const { t } = useTranslation();
  const { images, filing, refusal } = props.composer;
  return (
    <>
      <ImageList images={images.list} onRemove={images.remove} />
      {filing > 0 && <p className="filing">{t('composer.filing', { count: filing })}</p>}
      {refusal && (
        <p className="refusal" role="alert">
          {refusal}
        </p>
      )}
    </>
  );
}

export function Composer(props: ComposerProps) {
  const composer = useComposer({ node: props.node, keys: props.keys, deliver: typeInto(props.node), onTyped: props.onTyped });
  return (
    <ComposerFace name={props.name} dragging={composer.dragging} drop={composer.drop} field={composer.field} sendable={composer.sendable} onFiles={composer.onFiles} onSubmit={composer.onSubmit}>
      <ComposerNotes composer={composer} />
    </ComposerFace>
  );
}

function ComposerFace(props: {
  name: string;
  dragging: boolean;
  drop: Pick<DOMAttributes<HTMLDivElement>, 'onDragOver' | 'onDragLeave' | 'onDrop'>;
  field: Pick<ComponentProps<'textarea'>, 'ref' | 'value' | 'readOnly' | 'onChange' | 'onKeyDown' | 'onPaste'>;
  sendable: boolean;
  onFiles: (files: File[]) => void;
  onSubmit: () => void;
  children?: ReactNode;
}) {
  const { t } = useTranslation();
  return (
    <div className="composer" data-dragging={props.dragging || undefined} {...props.drop}>
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
        <FilePick onFiles={props.onFiles} />
        <button type="submit" className="send" aria-label={t('composer.send')} title={t('composer.send')} disabled={!props.sendable}>
          <ArrowUp aria-hidden="true" />
        </button>
      </form>
    </div>
  );
}

export function FilePick(props: { onFiles: (files: File[]) => void }) {
  const { t } = useTranslation();
  return (
    <label className="attach file" title={t('composer.attachFile')}>
      <Paperclip aria-hidden="true" />
      <input
        type="file"
        multiple
        aria-label={t('composer.attachFile')}
        onChange={(e) => {
          props.onFiles([...(e.target.files ?? [])]);
          e.target.value = '';
        }}
      />
    </label>
  );
}
