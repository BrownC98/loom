import { ChevronLeft, ChevronRight, Folder, Globe, LoaderCircle, Monitor, Terminal } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { connectSsh, getDistro, getFolders, getLocations, releaseSsh } from './api';
import type { FolderList } from './gen/FolderList';
import type { Locations } from './gen/Locations';
import type { PlaceView } from './gen/PlaceView';
import type { ReachView } from './gen/ReachView';
import { reachText, systemText, uniqueHosts, useStarting } from './place';

export function childPath(parent: string, name: string): string {
  const sep = parent.includes('\\') && !parent.includes('/') ? '\\' : '/';
  return parent.endsWith(sep) ? parent + name : parent + sep + name;
}

const distroOf = (list: FolderList | null) => (list?.place.kind === 'wsl' ? list.place.distro : null);
const hostOf = (list: FolderList | null) => (list?.place.kind === 'ssh' ? list.place.host : null);

export function FolderPicker(props: { id: string; label: string; disabled: boolean; autoFocus?: boolean; hosts: string[]; onPick: (path: string) => void; onPlace: (place: PlaceView | null) => void }) {
  const { t } = useTranslation();
  const [dir, setDir] = useState('');
  const [list, setList] = useState<FolderList | null>(null);
  const [failed, setFailed] = useState('');
  const [selected, setSelected] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const [locations, setLocations] = useState<Locations | null>(null);
  const [waiting, setWaiting] = useState<string | null>(null);
  const [asking, setAsking] = useState(false);
  const [host, setHost] = useState('');
  const [connecting, setConnecting] = useState<string | null>(null);
  const [reach, setReach] = useState<{ host: string; view: ReachView } | null>(null);
  const starting = useStarting(waiting !== null);
  const latest = useRef(0);
  const visited = useRef(new Set<string>());
  const inflight = useRef(new Set<Promise<unknown>>());
  const { onPick, onPlace } = props;

  function track<T>(request: Promise<T>): Promise<T> {
    inflight.current.add(request);
    const done = () => inflight.current.delete(request);
    request.then(done, done);
    return request;
  }
  const places = (locations?.distros.length ?? 0) > 0;
  const reached = hostOf(list);
  const hosts = uniqueHosts(reached === null ? props.hosts : [...props.hosts, reached]);

  async function load(path?: string) {
    const ask = ++latest.current;
    try {
      const asked = path?.match(/^ssh:\/\/([^/]+)/i)?.[1];
      if (asked) visited.current.add(asked);
      const next = await track(getFolders(path));
      if (ask !== latest.current) return;
      setList(next);
      setFailed('');
      setWaiting(null);
      if (next.place.kind === 'ssh') setAsking(false);
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
    const hosts = visited.current;
    const requests = inflight.current;
    return () => {
      latest.current++;
      Promise.allSettled([...requests]).then(() => {
        const names = [...hosts];
        hosts.clear();
        names.forEach((name) => releaseSsh(name).catch(() => {}));
      });
    };
  }, []);

  useEffect(() => {
    onPlace(list && (places || list.place.kind !== 'pc') ? list.place : null);
  }, [places, list]);

  async function goHome(distro: string) {
    const ask = ++latest.current;
    setAsking(false);
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
    setConnecting(null);
    setReach(null);
    setDir(path);
    setSelected(null);
    onPick(path);
    setList(null);
    setFailed('');
    setWaiting(distro);
    if (path.trim() === '') latest.current++;
    else load(path.trim());
  }

  function askHost() {
    setAsking(true);
    setOpen(true);
    go('');
  }

  async function connect(name: string) {
    const at = name.trim();
    if (at === '') return;
    const ask = ++latest.current;
    setOpen(true);
    setList(null);
    setFailed('');
    setReach(null);
    setConnecting(at);
    visited.current.add(at);
    try {
      const view = await track(connectSsh(at));
      if (ask !== latest.current) return;
      setConnecting(null);
      if (view.kind === 'reached') go(`ssh://${at}${view.home}`);
      setReach({ host: at, view });
    } catch (err) {
      if (ask !== latest.current) return;
      setConnecting(null);
      setFailed((err as Error).message);
    }
  }

  const drivePressed = (drive: string) => !asking && list !== null && list.place.kind === 'pc' && list.path.toUpperCase().startsWith(drive.toUpperCase());
  const distroPressed = (distro: string) => !asking && (distroOf(list) ?? waiting) === distro;
  const hostPressed = (name: string) => !asking && dir.toLowerCase().startsWith(`ssh://${name.toLowerCase()}/`);

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
          placeholder={asking ? t('place.sshPick') : undefined}
          spellCheck={false}
          autoComplete="off"
          autoFocus={props.autoFocus}
          translate="no"
        />
      </div>
      <div className="chips" role="group" aria-label={t('place.chips')}>
        {places &&
          locations!.drives.map((drive, i) => (
            <button key={drive} type="button" className="chip" aria-pressed={drivePressed(drive)} disabled={props.disabled} onClick={() => (setAsking(false), setOpen(true), go(drive))}>
              {i === 0 && <Monitor />}
              <span className="mono">{drive}</span>
            </button>
          ))}
        {places &&
          locations!.distros.map((distro) => (
            <button key={distro} type="button" className="chip" aria-pressed={distroPressed(distro)} disabled={props.disabled} onClick={() => goHome(distro)}>
              <Terminal />
              {t('place.wslDistro', { distro })}
            </button>
          ))}
        {hosts.map((name) => (
          <button key={name} type="button" className="chip" aria-pressed={hostPressed(name)} disabled={props.disabled} onClick={() => (setAsking(false), connect(name))} translate="no">
            <Globe />
            {t('place.sshHost', { host: name })}
          </button>
        ))}
        <button type="button" className="chip" aria-pressed={asking} disabled={props.disabled} onClick={askHost}>
          <Globe />
          {t('place.ssh')}
        </button>
      </div>
      {asking && (
        <div className="ssh-host">
          <div className="field">
            <label htmlFor={`${props.id}-host`}>{t('place.hostField')}</label>
            <div className="inrow">
              <input
                id={`${props.id}-host`}
                className="mono"
                value={host}
                onChange={(e) => setHost(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key !== 'Enter') return;
                  e.preventDefault();
                  connect(host);
                }}
                disabled={props.disabled}
                spellCheck={false}
                autoComplete="off"
                autoFocus
                translate="no"
              />
              <button type="button" className="btn primary" disabled={props.disabled || host.trim() === '' || connecting !== null} onClick={() => connect(host)}>
                {connecting !== null && <LoaderCircle className="spin" />}
                {t(connecting === null ? 'place.connect' : 'place.connecting')}
              </button>
            </div>
          </div>
          <p className="hint">{t('place.sshHint')}</p>
        </div>
      )}
      {connecting !== null && <p className="hint">{t('place.connectingHint', { host: connecting })}</p>}
      {reach !== null && reach.view.kind !== 'reached' && (
        <p className="error" role="alert">
          {reachText(reach.host, reach.view, t)}
        </p>
      )}
      {open && (
        <ul className="dirs" aria-label={t('repo.folders')} translate="no">
          {failed !== '' && <li className="hint">{failed}</li>}
          {failed === '' && list === null && <li className="hint">{starting ? t('place.starting', { distro: waiting }) : dir.trim() === '' ? t(asking ? 'place.sshHome' : 'repo.noFolders') : t('repo.loading')}</li>}
          {failed === '' && list?.parent != null && (
            <li>
              <button type="button" className="dir" disabled={props.disabled} aria-label={t('repo.up')} onClick={() => go(list.parent!, distroOf(list))}>
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
                  <button type="button" className="btn ghost sm" disabled={props.disabled} aria-label={t('repo.open', { name: folder.name })} title={t('repo.open', { name: folder.name })} onClick={() => go(path, distroOf(list))}>
                    <ChevronRight />
                  </button>
                </li>
              );
            })}
          {failed === '' && list?.folders.length === 0 && <li className="hint">{t('repo.noFolders')}</li>}
        </ul>
      )}
      {reach !== null && reach.view.kind === 'reached' && reached === reach.host && (
        <p className="hint" translate="no">
          {t('place.sshReached', { host: reach.host, system: systemText(reach.view.system, t) })}
        </p>
      )}
    </>
  );
}
