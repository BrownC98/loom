import type { TFunction } from 'i18next';
import { RotateCw } from 'lucide-react';
import { createContext, useContext, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { reconnectSsh } from './api';
import i18next from './i18n';
import type { PlaceView } from './gen/PlaceView';
import type { ReachView } from './gen/ReachView';
import type { RepoView } from './gen/RepoView';
import type { SystemView } from './gen/SystemView';

const STARTING_MS = 800;

export function placeLabel(place: PlaceView, t: TFunction): string | null {
  switch (place.kind) {
    case 'pc':
      return null;
    case 'wsl':
      return t('place.wslDistro', { distro: place.distro });
    case 'ssh':
      return t('place.sshHost', { host: place.host });
  }
}

export type Unreached = Exclude<ReachView, { kind: 'reached' }>;

export const Reaches = createContext<ReadonlyMap<string, ReachView | null>>(new Map());

export const sshHosts = (repos: readonly Pick<RepoView, 'place'>[]) => [...new Set(repos.flatMap((r) => (r.place.kind === 'ssh' ? [r.place.host] : [])))];

export function unreachedAt(place: PlaceView, reaches: ReadonlyMap<string, ReachView | null>): Unreached | null {
  if (place.kind !== 'ssh') return null;
  const reach = reaches.get(place.host);
  return reach == null || reach.kind === 'reached' ? null : reach;
}

export function reachText(host: string, reach: Unreached, t: TFunction): string {
  switch (reach.kind) {
    case 'unknownHostKey':
      return t('place.unknownHostKey', { host, folder: reach.folder });
    case 'authFailed':
      return t('place.authFailed', { host, folder: reach.folder });
    case 'unreachable':
      return t('place.unreachable', { host, cause: reach.cause });
    case 'unsupportedMac':
      return t('place.unsupportedMac', { host, cpu: reach.cpu });
    case 'versionMismatch':
      return t('place.versionMismatch', { host });
    case 'failed':
      return t('place.failed', { host, cause: reach.cause });
  }
}

export function systemText(system: SystemView, t: TFunction): string {
  switch (system.kind) {
    case 'linux':
      return t('place.linux');
    case 'mac':
      return t('place.mac');
    case 'windows':
      return t('place.windowsWsl', { distro: system.distro });
  }
}

export function PlaceTag({ place }: { place: PlaceView }) {
  const { t } = useTranslation();
  const down = unreachedAt(place, useContext(Reaches));
  const label = placeLabel(place, t);
  if (label === null) return null;
  if (down !== null && place.kind === 'ssh')
    return (
      <span className="place down" translate="no" title={reachText(place.host, down, t)}>
        {t('place.downTag', { place: label })}
      </span>
    );
  return (
    <span className="place" translate="no">
      {label}
    </span>
  );
}

export function ShortPlaceTag({ place }: { place: PlaceView }) {
  const { t } = useTranslation();
  const down = unreachedAt(place, useContext(Reaches));
  const label = placeLabel(place, t);
  if (label === null) return null;
  if (down !== null && place.kind === 'ssh')
    return (
      <span className="place down" translate="no" title={reachText(place.host, down, t)}>
        {t('place.downTag', { place: place.host })}
      </span>
    );
  return (
    <span className="place" translate="no" title={label}>
      {place.kind === 'wsl' ? t('place.wsl') : label}
    </span>
  );
}

export function DownLine({ place }: { place: PlaceView }) {
  const { t } = useTranslation();
  const down = unreachedAt(place, useContext(Reaches));
  const [asking, setAsking] = useState(false);
  if (down === null || place.kind !== 'ssh') return null;
  const reconnect = () => {
    setAsking(true);
    reconnectSsh(place.host)
      .catch(() => {})
      .finally(() => setAsking(false));
  };
  return (
    <p className="downline nodrag" role="status" translate="no">
      <span>
        {reachText(place.host, down, t)} {t('place.downKeep', { host: place.host })}
      </span>
      <button type="button" className="btn sm" disabled={asking} onClick={reconnect}>
        <RotateCw />
        {t('place.reconnect')}
      </button>
    </p>
  );
}

export const wslDistro = (repo: Pick<RepoView, 'place'> | null | undefined) => (repo?.place.kind === 'wsl' ? repo.place.distro : null);

export function useStarting(busy: boolean): boolean {
  const [slow, setSlow] = useState(false);
  useEffect(() => {
    setSlow(false);
    if (!busy) return;
    const timer = window.setTimeout(() => setSlow(true), STARTING_MS);
    return () => clearTimeout(timer);
  }, [busy]);
  return busy && slow;
}

export const placeName = (distro: string | null) => (distro === null ? i18next.t('place.pc') : i18next.t('place.wslDistro', { distro }));

export const shortPlaceName = (distro: string | null) => (distro === null ? i18next.t('place.pcShort') : i18next.t('place.wslDistro', { distro }));
