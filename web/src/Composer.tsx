import { ArrowUp, Image as ImageIcon, Paperclip, SquareTerminal } from 'lucide-react';
import { useEffect, useLayoutEffect, useRef, useState, type ClipboardEvent, type DragEvent, type KeyboardEvent } from 'react';
import { useTranslation } from 'react-i18next';
import { savePastedFile } from './api';
import { droppedFiles, ImageList, isImage, quotePath, tooBig, useImages } from './attachments';
import { IME_KEY_CODE, watchComposition, type Composition } from './composition';
import { useDraft } from './drafts';
import type { NodeId } from './gen/NodeId';
import { useNodeInput } from './nodeInputs';

export type ComposerKeys = { sends: (e: KeyboardEvent<HTMLTextAreaElement>) => boolean; escape: (() => void) | null };

export const deskKeys = (escape: () => void): ComposerKeys => ({
  sends: (e) => e.key === 'Enter' && !e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey && !e.nativeEvent.isComposing && e.keyCode !== IME_KEY_CODE,
  escape,
});

export const PHONE_KEYS: ComposerKeys = { sends: () => false, escape: null };

const COMPOSER_ROOM = '--composer-room';

type ComposerProps = { node: NodeId; name: string; keys: ComposerKeys; onTyped?: (before: string, after: string) => boolean };

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
  const input = useNodeInput(props.node);
  const [text, setText] = useDraft(`composer:${props.node}`);
  const [refusal, setRefusal] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const [dragging, setDragging] = useState(false);
  const images = useImages(`composer:${props.node}`, props.node, setRefusal);
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
  const ready = text.trim() !== '' || images.list.length > 0;

  const insertPath = (path: string) => {
    const el = area.current;
    const now = el?.value ?? text;
    const at = el?.selectionStart ?? now.length;
    const typed = `${at > 0 && !/\s/.test(now[at - 1]) ? ' ' : ''}${quotePath(path)} `;
    setText(now.slice(0, at) + typed + now.slice(at));
    requestAnimationFrame(() => el?.setSelectionRange(at + typed.length, at + typed.length));
  };
  const take = (files: File[]) => {
    setRefusal(null);
    images.attach(files.filter(isImage));
    for (const file of files.filter((f) => !isImage(f))) {
      const big = tooBig(file);
      if (big) setRefusal(big);
      else savePastedFile(file, props.node).then(insertPath, (err: Error) => setRefusal(err.message));
    }
  };

  const send = () => {
    const body = area.current?.value ?? text;
    if (body.trim() === '' && images.list.length === 0) return;
    setRefusal(null);
    setSending(true);
    images
      .paths()
      .then((paths) => {
        if (!input) throw new Error(t('composer.gone'));
        input.paste([body.trimEnd(), paths.map(quotePath).join(' ')].filter((part) => part !== '').join('\n'));
        input.enter();
        setText('');
        images.clear();
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
