import { ChevronLeft, ChevronRight, Folder, Monitor, Terminal } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { getDistro, getFolders, getLocations } from './api';
import type { FolderList } from './gen/FolderList';
import type { Locations } from './gen/Locations';
import { useStarting } from './place';

export function childPath(parent: string, name: string): string {
  const sep = parent.includes('\\') && !parent.includes('/') ? '\\' : '/';
  return parent.endsWith(sep) ? parent + name : parent + sep + name;
}

export function FolderPicker(props: { id: string; label: string; disabled: boolean; autoFocus?: boolean; onPick: (path: string) => void; onPlace: (place: string | null) => void }) {
  const { t } = useTranslation();
  const [dir, setDir] = useState('');
  const [list, setList] = useState<FolderList | null>(null);
  const [failed, setFailed] = useState('');
  const [selected, setSelected] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const [locations, setLocations] = useState<Locations | null>(null);
  const [waiting, setWaiting] = useState<string | null>(null);
  const starting = useStarting(waiting !== null);
  const latest = useRef(0);
  const { onPick, onPlace } = props;
  const places = (locations?.distros.length ?? 0) > 0;

  async function load(path?: string) {
    const ask = ++latest.current;
    try {
      const next = await getFolders(path);
      if (ask !== latest.current) return;
      setList(next);
      setFailed('');
      setWaiting(null);
      if (path === undefined) {
        setDir(next.path);
        onPick(next.path);
      }
    } catch (err) {
      if (ask !== latest.current) return;
      setFailed((err as Error).message);
      setWaiting(null);
    }
  }

  useEffect(() => {
    load();
    getLocations().then(setLocations, () => {});
  }, []);

  useEffect(() => {
    onPlace(places && list ? (list.distro === null ? t('place.pc') : t('place.wslDistro', { distro: list.distro })) : null);
  }, [places, list, t]);

  async function goHome(distro: string) {
    const ask = ++latest.current;
    setOpen(true);
    setList(null);
    setFailed('');
    setWaiting(distro);
    try {
      const { home } = await getDistro(distro);
      if (ask === latest.current) go(home, distro);
    } catch (err) {
      if (ask !== latest.current) return;
      setFailed((err as Error).message);
      setWaiting(null);
    }
  }

  function go(path: string, distro: string | null = null) {
    setDir(path);
    setSelected(null);
    onPick(path);
    setList(null);
    setFailed('');
    setWaiting(distro);
    if (path.trim() === '') latest.current++;
    else load(path.trim());
  }

  const drivePressed = (drive: string) => list !== null && list.distro === null && list.path.toUpperCase().startsWith(drive.toUpperCase());
  const distroPressed = (distro: string) => (list?.distro ?? waiting) === distro;

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
      {places && (
        <div className="chips" role="group" aria-label={t('place.chips')}>
          {locations!.drives.map((drive, i) => (
            <button key={drive} type="button" className="chip" aria-pressed={drivePressed(drive)} disabled={props.disabled} onClick={() => (setOpen(true), go(drive))}>
              {i === 0 && <Monitor />}
              <span className="mono">{drive}</span>
            </button>
          ))}
          {locations!.distros.map((distro) => (
            <button key={distro} type="button" className="chip" aria-pressed={distroPressed(distro)} disabled={props.disabled} onClick={() => goHome(distro)}>
              <Terminal />
              {t('place.wslDistro', { distro })}
            </button>
          ))}
        </div>
      )}
      {open && (
        <ul className="dirs" aria-label={t('repo.folders')} translate="no">
          {failed !== '' && <li className="hint">{failed}</li>}
          {failed === '' && list === null && <li className="hint">{starting ? t('place.starting', { distro: waiting }) : dir.trim() === '' ? t('repo.noFolders') : t('repo.loading')}</li>}
          {failed === '' && list?.parent != null && (
            <li>
              <button type="button" className="dir" disabled={props.disabled} aria-label={t('repo.up')} onClick={() => go(list.parent!, list.distro)}>
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
                    <Folder />
                    <span className="dir-name">{folder.name}</span>
                    {folder.git && <span className="tag">git</span>}
                  </button>
                  <button type="button" className="btn ghost sm" disabled={props.disabled} aria-label={t('repo.open', { name: folder.name })} title={t('repo.open', { name: folder.name })} onClick={() => go(path, list.distro)}>
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
