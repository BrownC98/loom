import { X } from 'lucide-react';
import { useState, useSyncExternalStore } from 'react';
import { useTranslation } from 'react-i18next';
import { PASTE_LIMIT_BYTES, savePastedFile } from './api';
import type { NodeId } from './gen/NodeId';
import i18next from './i18n';
import { Modal } from './Modal';
import { PinchImage } from './PinchImage';

export const MB = 1024 * 1024;

export function quotePath(path: string): string {
  return `'${path.replaceAll("'", "'\\''")}'`;
}

export const isImage = (file: File) => file.type.startsWith('image/');

export function tooBig(file: File): string | null {
  if (file.size <= PASTE_LIMIT_BYTES) return null;
  return i18next.t('terminal.tooBig', { name: file.name, size: Math.ceil(file.size / MB), limit: PASTE_LIMIT_BYTES / MB });
}

export function droppedFiles(data: DataTransfer): { files: File[]; errors: string[] } {
  const files: File[] = [];
  const errors: string[] = [];
  for (const item of [...data.items]) {
    const file = item.kind === 'file' ? item.getAsFile() : null;
    if (!file) continue;
    const big = tooBig(file);
    if (item.webkitGetAsEntry()?.isDirectory) errors.push(i18next.t('terminal.folder', { name: file.name }));
    else if (big) errors.push(big);
    else files.push(file);
  }
  return { files, errors };
}

type Upload = 'busy' | 'saved' | 'failed';

export type AttachedImage = { id: number; url: string; name: string; upload: Upload; saved: Promise<string | null> };

const NONE: readonly AttachedImage[] = [];
const lists = new Map<string, readonly AttachedImage[]>();
const heard = new Set<() => void>();
let nextId = 0;

const listen = (heed: () => void) => {
  heard.add(heed);
  return () => void heard.delete(heed);
};
const write = (key: string, next: readonly AttachedImage[]) => {
  if (next.length > 0) lists.set(key, next);
  else lists.delete(key);
  heard.forEach((heed) => heed());
};
const edit = (key: string, change: (list: readonly AttachedImage[]) => readonly AttachedImage[]) => write(key, change(lists.get(key) ?? NONE));

export function useImages(key: string, node: NodeId, onError: (message: string) => void) {
  const { t } = useTranslation();
  const list = useSyncExternalStore(listen, () => lists.get(key) ?? NONE);
  const attach = (files: File[]) => {
    for (const file of files) {
      const big = tooBig(file);
      if (!isImage(file)) onError(t('inbox.notImage', { name: file.name }));
      else if (big) onError(big);
      else {
        const id = nextId++;
        const mark = (upload: Upload) => edit(key, (now) => now.map((i) => (i.id === id ? { ...i, upload } : i)));
        const saved = savePastedFile(file, node).then(
          (path) => (mark('saved'), path),
          (err: Error) => (mark('failed'), onError(err.message), null),
        );
        edit(key, (now) => [...now, { id, url: URL.createObjectURL(file), name: file.name, upload: 'busy', saved }]);
      }
    }
  };
  const remove = (image: AttachedImage) => {
    URL.revokeObjectURL(image.url);
    edit(key, (now) => now.filter((i) => i.id !== image.id));
  };
  const clear = () => {
    list.forEach((i) => URL.revokeObjectURL(i.url));
    edit(key, (now) => now.filter((i) => !list.includes(i)));
  };
  const paths = () => Promise.all(list.map((i) => i.saved)).then((all) => all.filter((p): p is string => p !== null));
  return { list, attach, remove, clear, paths };
}

export function ZoomImage(props: { src: string; name: string; title: string }) {
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

export function ImageList(props: { images: readonly AttachedImage[]; onRemove: (image: AttachedImage) => void }) {
  const { t } = useTranslation();
  if (props.images.length === 0) return null;
  return (
    <ul className="answer-images">
      {props.images.map((image) => (
        <li key={image.id} data-upload={image.upload}>
          <ZoomImage src={image.url} name={image.name} title={image.name} />
          {image.upload !== 'saved' && <span className="upload">{t(image.upload === 'busy' ? 'composer.uploading' : 'composer.failed')}</span>}
          {image.upload !== 'busy' && (
            <button type="button" aria-label={t('inbox.removeImage')} title={t('inbox.removeImage')} onClick={() => props.onRemove(image)}>
              <X aria-hidden="true" />
            </button>
          )}
        </li>
      ))}
    </ul>
  );
}
