import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import i18next from './i18n';
import type { RepoView } from './gen/RepoView';
import type { WslPlace } from './gen/WslPlace';

const STARTING_MS = 800;

export function PlaceTag({ wsl }: { wsl: WslPlace | null }) {
  const { t } = useTranslation();
  if (!wsl) return null;
  return (
    <span className="place" translate="no">
      {t('place.wslDistro', { distro: wsl.distro })}
    </span>
  );
}

export function ShortPlaceTag({ wsl }: { wsl: WslPlace | null }) {
  const { t } = useTranslation();
  if (!wsl) return null;
  return (
    <span className="place" translate="no" title={t('place.wslDistro', { distro: wsl.distro })}>
      {t('place.wsl')}
    </span>
  );
}

export const shownPath = (repo: Pick<RepoView, 'path' | 'wsl'>) => repo.wsl?.path ?? repo.path;

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
