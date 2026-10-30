import { ArrowUp, Image as ImageIcon, Paperclip, SquareTerminal } from 'lucide-react';
import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore, type ClipboardEvent, type DragEvent, type KeyboardEvent } from 'react';
import { useTranslation } from 'react-i18next';
import { savePastedFile } from './api';
import { droppedFiles, ImageList, isImage, quotePath, tooBig, useImages } from './attachments';
import { IME_KEY_CODE, watchComposition, type Composition } from './composition';
import { editDraft, readDraft, useDraft } from './drafts';
import type { NodeId } from './gen/NodeId';
import { nodeInput, useNodeInput, type NodeInput } from './nodeInputs';

export type ComposerKeys = { sends: (e: KeyboardEvent<HTMLTextAreaElement>) => boolean; escape: (() => void) | null; blank: ((input: NodeInput) => void) | null };

export const deskKeys = (escape: () => void): ComposerKeys => ({
  sends: (e) => e.key === 'Enter' && !e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey && !e.nativeEvent.isComposing && e.keyCode !== IME_KEY_CODE,
  escape,
  blank: null,
});

export const PHONE_KEYS: ComposerKeys = { sends: () => false, escape: null, blank: (input) => input.enter() };

const COMPOSER_ROOM = '--composer-room';

type ComposerProps = { node: NodeId; name: string; keys: ComposerKeys; onTyped?: (before: string, after: string) => boolean };

const uploads = new Map<string, ReadonlySet<Promise<unknown>>>();
const heardUploads = new Set<() => void>();

const listenUploads = (heed: () => void) => {
  heardUploads.add(heed);
  return () => void heardUploads.delete(heed);
};
const setUploads = (key: string, next: ReadonlySet<Promise<unknown>>) => {
  if (next.size > 0) uploads.set(key, next);
  else uploads.delete(key);
  heardUploads.forEach((heed) => heed());
};

function holdUpload(key: string, upload: Promise<unknown>) {
  setUploads(key, new Set([...(uploads.get(key) ?? []), upload]));
  void upload.finally(() => setUploads(key, new Set([...(uploads.get(key) ?? [])].filter((u) => u !== upload))));
}

const uploadsDone = (key: string) => Promise.all([...(uploads.get(key) ?? [])]);

const useUploads = (key: string) => useSyncExternalStore(listenUploads, () => uploads.get(key)?.size ?? 0);

export function CanvasComposer(props: ComposerProps) {
  const input = useNodeInput(props.node);
  const box = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const el = box.current;
    const canvas = el?.parentElement;
    if (!el || !canvas) return;
    const room = () => canvas.style.setProperty(COMPOSER_ROOM, `${el.offsetHeight + 14}px`);
    room();
    const watch = new ResizeObserver(room);
    watch.observe(el);
    return () => {
      watch.disconnect();
      canvas.style.removeProperty(COMPOSER_ROOM);
    };
  }, [input !== null]);
  if (!input) return null;
  return (
    <div ref={box} className="composer-float">
      <Composer {...props} />
    </div>
  );
}

export function Composer(props: ComposerProps) {
  const { t } = useTranslation();
  const key = `composer:${props.node}`;
  const [text, setText] = useDraft(key);
  const [refusal, setRefusal] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const [dragging, setDragging] = useState(false);
  const images = useImages(key, props.node, setRefusal);
  const area = useRef<HTMLTextAreaElement>(null);
  const composition = useRef<Composition | null>(null);
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
  const filing = useUploads(key);
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
    setSending(true);
    const attached = images.list.length;
    Promise.all([images.paths(), uploadsDone(key)])
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
      .finally(() => setSending(false));
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
  const picker = (accept: string | undefined, label: string, Icon: typeof ImageIcon, className: string) => (
    <label className={`attach ${className}`} title={label}>
      <Icon aria-hidden="true" />
      <input
        type="file"
        accept={accept}
        multiple
        aria-label={label}
        onChange={(e) => {
          take([...(e.target.files ?? [])]);
          e.target.value = '';
        }}
      />
    </label>
  );

  return (
    <div
      className="composer"
      data-dragging={dragging || undefined}
      onDragOver={(e) => {
        if (!hasFiles(e)) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = 'copy';
        setDragging(true);
      }}
      onDragLeave={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDragging(false);
      }}
      onDrop={onDrop}
    >
      <div className="composer-to">
        <span className="target">
          <SquareTerminal aria-hidden="true" />
          <b>{props.name}</b>
          {t('composer.to')}
        </span>
        <span className="keys">{t('composer.keys')}</span>
      </div>
      <ImageList images={images.list} onRemove={images.remove} />
      {filing > 0 && <p className="filing">{t('composer.filing', { count: filing })}</p>}
      {refusal && (
        <p className="refusal" role="alert">
          {refusal}
        </p>
      )}
      <form
        className="composer-line"
        onSubmit={(e) => {
          e.preventDefault();
          if (ready && !sending) submit();
        }}
      >
        <textarea
          ref={area}
          rows={1}
          value={text}
          readOnly={sending}
          onChange={(e) => {
            if (!props.onTyped?.(text, e.target.value)) setText(e.target.value);
          }}
          onKeyDown={onKeyDown}
          onPaste={onPaste}
          aria-label={t('composer.label', { name: props.name })}
          placeholder={t('composer.placeholder')}
          autoComplete="off"
          autoCapitalize="off"
          spellCheck={false}
        />
        {picker('image/*', t('composer.attachImage'), ImageIcon, 'image')}
        {picker(undefined, t('composer.attachFile'), Paperclip, 'file')}
        <button type="submit" className="send" aria-label={t('composer.send')} title={t('composer.send')} disabled={!ready || sending}>
          <ArrowUp aria-hidden="true" />
        </button>
      </form>
    </div>
  );
}
