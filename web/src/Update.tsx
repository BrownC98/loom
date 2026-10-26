import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { isTauri } from '@tauri-apps/api/core';
import { getVersion } from '@tauri-apps/api/app';
import { relaunch } from '@tauri-apps/plugin-process';
import { check, type Update as Release } from '@tauri-apps/plugin-updater';
import { checkUpdate, getUpdate, installUpdate } from './api';
import type { UpdateView } from './gen/UpdateView';

type Step =
  | { kind: 'available' }
  | { kind: 'downloading'; percent: number }
  | { kind: 'ready' }
  | { kind: 'restarting' }
  | { kind: 'error'; message: string; retry: () => void };

export interface Updates {
  checks: boolean;
  latest: string | null;
  step: Step | null;
  appVersion: string | null;
  check: () => Promise<string | null>;
  install: () => void;
  close: () => void;
}

const CHECK_EVERY_MS = 24 * 60 * 60 * 1000;
const RETRY_AFTER_MS = 60 * 60 * 1000;
const UPDATE_QUERY = 'update';

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

function stepOf(view: UpdateView, install: () => void): Step | null {
  const step = view.step;
  if (!step) return null;
  if (step.kind === 'failed') return { kind: 'error', message: step.message, retry: install };
  return step;
}

function useDaemonUpdates(viewVersion: number): Updates {
  const [view, setView] = useState<UpdateView>({ checks: false, latest: null, step: null });
  const [closed, setClosed] = useState<string | null>(null);
  useEffect(() => {
    let latest = true;
    getUpdate().then((next) => latest && setView(next), () => {});
    return () => {
      latest = false;
    };
  }, [viewVersion]);

  const install = () => {
    installUpdate().then(() => {}, (err: unknown) => setView((v) => ({ ...v, step: { kind: 'failed', message: String(err) } })));
  };
  const step = stepOf(view, install);
  return {
    checks: view.checks,
    latest: view.latest,
    step: step && step.kind !== closed ? step : null,
    appVersion: null,
    check: () => checkUpdate().then((next) => next.latest),
    install,
    close: () => setClosed(view.step?.kind ?? null),
  };
}

export const useUpdates: (viewVersion: number) => Updates = isTauri() ? useAppUpdates : useDaemonUpdates;

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
