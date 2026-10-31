import type { TFunction } from 'i18next';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import i18next from './i18n';
import type { PlaceView } from './gen/PlaceView';
import type { RepoView } from './gen/RepoView';

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

export function PlaceTag({ place }: { place: PlaceView }) {
  const { t } = useTranslation();
  const label = placeLabel(place, t);
  if (label === null) return null;
  return (
    <span className="place" translate="no">
      {label}
    </span>
  );
}

export function ShortPlaceTag({ place }: { place: PlaceView }) {
  const { t } = useTranslation();
  const label = placeLabel(place, t);
  if (label === null) return null;
  return (
    <span className="place" translate="no" title={label}>
      {place.kind === 'wsl' ? t('place.wsl') : label}
    </span>
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
