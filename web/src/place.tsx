// 실행 위치(이 PC, WSL 배포판)의 화면 조각. 위치는 데몬이 저장소 경로에서 읽어 `RepoView.wsl`로 준다. 근거: 목업 docs/mockups/wsl-place.html.
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import i18next from './i18n';
import type { RepoView } from './gen/RepoView';
import type { WslPlace } from './gen/WslPlace';

/** 이보다 오래 걸리는 WSL 요청은 배포판이 시작하는 중으로 보고 그 글을 보인다(⑥b). 켜진 배포판의 요청은 보통 0.3초 안에 끝난다. */
const STARTING_MS = 800;

/** 저장소 상자와 그래프 제목의 꼬리표(② A안): WSL 저장소에만 「WSL · 배포판」. */
export function PlaceTag({ wsl }: { wsl: WslPlace | null }) {
  const { t } = useTranslation();
  if (!wsl) return null;
  return (
    <span className="place" translate="no">
      {t('place.wslDistro', { distro: wsl.distro })}
    </span>
  );
}

/** 사이드바 꼬리표: 좁아서 「WSL」만 쓰고 배포판은 툴팁이다. */
export function ShortPlaceTag({ wsl }: { wsl: WslPlace | null }) {
  const { t } = useTranslation();
  if (!wsl) return null;
  return (
    <span className="place" translate="no" title={t('place.wslDistro', { distro: wsl.distro })}>
      {t('place.wsl')}
    </span>
  );
}

/** 저장소 경로 글. WSL 저장소는 배포판 안의 경로(`~/…`)다(②′). */
export const shownPath = (repo: Pick<RepoView, 'path' | 'wsl'>) => repo.wsl?.path ?? repo.path;

/** `busy`가 `STARTING_MS`보다 오래 이어지면 참이다. */
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

/** 위치 이름(계정 탭 머리 줄). `distro`가 null이면 이 PC다. */
export const placeName = (distro: string | null) => (distro === null ? i18next.t('place.pc') : i18next.t('place.wslDistro', { distro }));

/** 좁은 줄(에이전트 메타 줄, 새 노드 머리 줄)의 위치 이름. */
export const shortPlaceName = (distro: string | null) => (distro === null ? i18next.t('place.pcShort') : i18next.t('place.wslDistro', { distro }));
