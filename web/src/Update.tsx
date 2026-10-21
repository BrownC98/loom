import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { isTauri } from '@tauri-apps/api/core';
import { getVersion } from '@tauri-apps/api/app';
import { relaunch } from '@tauri-apps/plugin-process';
import { check, type Update as Release } from '@tauri-apps/plugin-updater';
import { checkUpdate, getUpdate, installUpdate } from './api';
import type { UpdateView } from './gen/UpdateView';

/** 카드가 보이는 단계. 오류는 실패한 단계를 다시 부를 함수를 든다. */
type Step =
  | { kind: 'available' }
  | { kind: 'downloading'; percent: number }
  | { kind: 'ready' }
  | { kind: 'restarting' }
  | { kind: 'error'; message: string; retry: () => void };

/** 알림 카드와 설정 › 정보가 함께 쓰는 업데이트 상태. App이 하나를 쥔다(docs/mockups/web-update.html). */
export interface Updates {
  /** 이 화면에서 확인할 수 있다. 개발 빌드의 데몬은 확인하지 않는다. */
  checks: boolean;
  /** 이 화면보다 새 판. */
  latest: string | null;
  /** 카드의 단계. 없거나 닫았으면 카드를 그리지 않는다. */
  step: Step | null;
  /** 앱 창의 앱 판. 브라우저에서는 없다. */
  appVersion: string | null;
  /** 지금 확인하고 새 판을 돌려준다. 최신이면 null이다. */
  check: () => Promise<string | null>;
  /** [업데이트]. 진행은 카드가 보인다. */
  install: () => void;
  close: () => void;
}

/** 앱 창의 다음 자동 확인까지의 시간. Orca(`src/main/updater-events.ts`)와 같다: 확인에 성공하면 하루, 실패하면 한 시간 뒤다. */
const CHECK_EVERY_MS = 24 * 60 * 60 * 1000;
const RETRY_AFTER_MS = 60 * 60 * 1000;
/** 데몬이 받거나 다시 뜨는 동안 상태를 다시 읽는 간격. 퍼센트는 `ViewChanged`로 오지 않는다. */
const POLL_MS = 1000;
/** 데몬이 앱을 `--update`로 띄우면 앱이 화면 주소에 붙이는 물음. 근거: `app/src/main.rs`. */
const UPDATE_QUERY = 'update';

/**
 * 앱 창: Tauri 업데이터(docs/mockups/update.html). 화면을 열 때와 그 뒤 하루마다 확인한다. 받기를 시작하면 더 확인하지 않는다.
 * 주소에 `?update`가 있으면 묻지 않고 받아 설치하고 다시 뜬다(브라우저에서 앱 안의 데몬을 업데이트, 목업 ②).
 * Windows: 설치는 설치 파일을 띄우고 앱을 끝낸다. 설치 파일이 새 판을 다시 띄운다. 그래서 `relaunch`에 닿지 않는다.
 * macOS: 설치는 앱 묶음(`loom.app`)을 바꾸고 돌아온다. 그 뒤 앱을 다시 띄운다.
 */
function useAppUpdates(): Updates {
  const [release, setRelease] = useState<Release | null>(null);
  const [step, setStep] = useState<Step | null>(null);
  const [appVersion, setAppVersion] = useState<string | null>(null);
  const next = useRef<ReturnType<typeof setTimeout>>(undefined);

  function fail(retry: () => void) {
    return (err: unknown) => setStep({ kind: 'error', message: String(err), retry });
  }

  async function find(): Promise<Release | null> {
    clearTimeout(next.current);
    try {
      const found = await check();
      setRelease(found);
      setStep(found && { kind: 'available' });
      next.current = setTimeout(find, CHECK_EVERY_MS);
      return found;
    } catch (err) {
      fail(find)(err);
      next.current = setTimeout(find, RETRY_AFTER_MS);
      throw err;
    }
  }

  function download(found: Release, then: (found: Release) => void) {
    clearTimeout(next.current);
    let total = 0;
    let done = 0;
    setStep({ kind: 'downloading', percent: 0 });
    found
      .download((event) => {
        if (event.event === 'Started') total = event.data.contentLength ?? 0;
        if (event.event !== 'Progress') return;
        done += event.data.chunkLength;
        if (total > 0) setStep({ kind: 'downloading', percent: Math.floor((done / total) * 100) });
      })
      .then(() => then(found), fail(() => download(found, then)));
  }

  function ready() {
    setStep({ kind: 'ready' });
  }

  function install(found: Release) {
    found.install().then(relaunch).catch(fail(() => install(found)));
  }

  useEffect(() => {
    getVersion().then(setAppVersion, () => {});
    const params = new URLSearchParams(location.search);
    if (params.has(UPDATE_QUERY)) {
      // 다시 읽어도(새 빌드) 또 설치하지 않게 주소에서 지운다.
      params.delete(UPDATE_QUERY);
      const query = params.toString();
      history.replaceState(null, '', `${location.pathname}${query && `?${query}`}${location.hash}`);
      find().then((found) => found && download(found, install), () => {});
    } else {
      find().catch(() => {});
    }
    return () => clearTimeout(next.current);
  }, []);

  return {
    checks: true,
    latest: release?.version ?? null,
    step,
    appVersion,
    check: () => find().then((found) => found?.version ?? null),
    install: () => {
      if (release && step?.kind === 'ready') install(release);
      else if (release) download(release, ready);
    },
    close: () => setStep(null),
  };
}

/** 데몬의 단계를 카드 단계로 옮긴다. 데몬의 실패는 [업데이트]를 다시 부르면 다시 받는다. */
function stepOf(view: UpdateView, install: () => void): Step | null {
  const step = view.step;
  if (!step) return null;
  if (step.kind === 'failed') return { kind: 'error', message: step.message, retry: install };
  return step;
}

/**
 * 브라우저: 데몬의 업데이트(`GET /api/update`). 데몬이 시작할 때와 6시간마다 확인하고, 바뀌면 `ViewChanged`가 온다(`viewVersion`).
 * 받거나 다시 뜨는 동안은 다시 읽는다. 데몬이 새 판으로 바뀌면 App이 새 빌드를 다시 읽는다(`newBuildServed`).
 */
function useDaemonUpdates(viewVersion: number): Updates {
  const [view, setView] = useState<UpdateView>({ checks: false, latest: null, step: null });
  // 닫은 단계. 데몬의 단계가 바뀌면 카드가 다시 보인다.
  const [closed, setClosed] = useState<string | null>(null);
  const read = () => getUpdate().then(setView, () => {});

  useEffect(() => {
    read();
  }, [viewVersion]);

  const busy = view.step?.kind === 'downloading' || view.step?.kind === 'restarting';
  useEffect(() => {
    if (!busy) return;
    const timer = setInterval(read, POLL_MS);
    return () => clearInterval(timer);
  }, [busy]);

  const install = () => {
    installUpdate().then(setView, (err: unknown) => setView((v) => ({ ...v, step: { kind: 'failed', message: String(err) } })));
  };
  const step = stepOf(view, install);
  return {
    checks: view.checks,
    latest: view.latest,
    step: step && step.kind !== closed ? step : null,
    appVersion: null,
    check: () =>
      checkUpdate().then((next) => {
        setView(next);
        return next.latest;
      }),
    install,
    close: () => setClosed(view.step?.kind ?? null),
  };
}

/** 앱 창은 앱 업데이터를, 브라우저는 데몬을 쓴다. 한 화면에서 바뀌지 않으므로 처음 한 번 고른다. */
export const useUpdates: (viewVersion: number) => Updates = isTauri() ? useAppUpdates : useDaemonUpdates;

/** 오른쪽 아래 업데이트 카드(docs/mockups/update.html, docs/mockups/web-update.html ①②). */
export function UpdateCard({ updates }: { updates: Updates }) {
  const { t } = useTranslation();
  const { step, latest } = updates;
  if (!step) return null;
  const version = latest && <p className="hint">{t('update.version', { version: latest })}</p>;
  return (
    <div className="pop update-pop" role="complementary" aria-label={t(`update.${step.kind}`)} aria-live="polite">
      <h3>
        {t(`update.${step.kind}`)}
        <span className="spacer" />
        <button type="button" className="btn ghost sm icon" aria-label={t('common.close')} onClick={updates.close}>
          ×
        </button>
      </h3>
      {step.kind === 'available' && (
        <>
          {version}
          <p className="hint">{t('update.keep')}</p>
          <button type="button" className="btn primary" onClick={updates.install}>
            {t('update.update')}
          </button>
        </>
      )}
      {step.kind === 'downloading' && (
        <>
          {version}
          <span className="meter">
            <i style={{ width: `${step.percent}%` }} />
          </span>
          <p className="hint">{t('update.percent', { percent: step.percent })}</p>
        </>
      )}
      {step.kind === 'ready' && (
        <>
          {version}
          <p className="hint">{t('update.keep')}</p>
          <button type="button" className="btn primary" onClick={updates.install}>
            {t('update.restart')}
          </button>
        </>
      )}
      {step.kind === 'restarting' && (
        <>
          {version}
          <p className="hint">{t('update.restartingHint')}</p>
        </>
      )}
      {step.kind === 'error' && (
        <>
          <p className="error">{step.message}</p>
          <button type="button" className="btn primary" onClick={step.retry}>
            {t('update.retry')}
          </button>
        </>
      )}
    </div>
  );
}
