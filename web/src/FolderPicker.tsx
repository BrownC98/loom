import { ChevronLeft, ChevronRight } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { getFolders } from './api';
import type { FolderList } from './gen/FolderList';

/** `parent` 안의 `name`. 데몬의 경로 구분자(Windows면 `\`)를 따른다. */
export function childPath(parent: string, name: string): string {
  const sep = parent.includes('\\') && !parent.includes('/') ? '\\' : '/';
  return parent.endsWith(sep) ? parent + name : parent + sep + name;
}

/**
 * 데몬이 보는 폴더를 고른다(목업 repository.html 1). 경로 칸에 적거나 목록에서 고른다. 칸을 누르면 목록이 열린다.
 * 고른 값은 목록에서 고른 폴더, 없으면 칸의 폴더다. 처음 폴더는 데몬이 정한다(`<홈>/loom`).
 */
export function FolderPicker(props: { id: string; label: string; disabled: boolean; autoFocus?: boolean; onPick: (path: string) => void }) {
  const { t } = useTranslation();
  const [dir, setDir] = useState('');
  const [list, setList] = useState<FolderList | null>(null);
  const [failed, setFailed] = useState('');
  const [selected, setSelected] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const latest = useRef(0);
  const { onPick } = props;

  async function load(path?: string) {
    const ask = ++latest.current;
    try {
      const next = await getFolders(path);
      if (ask !== latest.current) return;
      setList(next);
      setFailed('');
      if (path === undefined) {
        setDir(next.path);
        onPick(next.path);
      }
    } catch (err) {
      if (ask === latest.current) setFailed((err as Error).message);
    }
  }

  useEffect(() => {
    load();
  }, []);

  /** 칸을 고칠 때마다 앞 요청을 버리고 옛 목록을 지운다. 늦은 응답이 적은 글이나 고른 폴더를 덮지 않는다. */
  function go(path: string) {
    setDir(path);
    setSelected(null);
    onPick(path);
    setList(null);
    setFailed('');
    if (path.trim() === '') latest.current++;
    else load(path.trim());
  }

  function select(path: string) {
    setSelected(path);
    onPick(path);
  }

  return (
    <>
      <div className="field">
        <label htmlFor={props.id}>{props.label}</label>
        <input
          id={props.id}
          className="mono"
          value={dir}
          onChange={(e) => go(e.target.value)}
          onFocus={() => setOpen(true)}
          onClick={() => setOpen(true)}
          disabled={props.disabled}
          spellCheck={false}
          autoComplete="off"
          autoFocus={props.autoFocus}
          translate="no"
        />
      </div>
      {open && (
        <ul className="dirs" aria-label={t('repo.folders')} translate="no">
          {failed !== '' && <li className="hint">{failed}</li>}
          {failed === '' && list === null && <li className="hint">{dir.trim() === '' ? t('repo.noFolders') : t('repo.loading')}</li>}
          {failed === '' && list?.parent != null && (
            <li>
              <button type="button" className="dir" disabled={props.disabled} aria-label={t('repo.up')} onClick={() => go(list.parent!)}>
                <ChevronLeft />
                ..
              </button>
            </li>
          )}
          {failed === '' &&
            list?.folders.map((folder) => {
              const path = childPath(list.path, folder.name);
              return (
                <li key={folder.name} className={selected === path ? 'sel' : undefined}>
                  <button type="button" className="dir" disabled={props.disabled} aria-pressed={selected === path} onClick={() => select(path)}>
                    <svg viewBox="0 0 24 24" aria-hidden="true">
                      <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
                    </svg>
                    <span className="dir-name">{folder.name}</span>
                    {folder.git && <span className="tag">git</span>}
                  </button>
                  <button type="button" className="btn ghost sm" disabled={props.disabled} aria-label={t('repo.open', { name: folder.name })} title={t('repo.open', { name: folder.name })} onClick={() => go(path)}>
                    <ChevronRight />
                  </button>
                </li>
              );
            })}
          {failed === '' && list?.folders.length === 0 && <li className="hint">{t('repo.noFolders')}</li>}
        </ul>
      )}
    </>
  );
}
