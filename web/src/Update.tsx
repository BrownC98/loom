import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { isTauri } from '@tauri-apps/api/core';
import { check, type Update as Release } from '@tauri-apps/plugin-updater';

/** 카드가 보이는 단계. 오류는 실패한 단계를 다시 부를 함수를 든다. */
type Step =
  | { kind: 'available' }
  | { kind: 'downloading'; percent: number }
  | { kind: 'ready' }
  | { kind: 'error'; message: string; retry: () => void };

/** 다음 자동 확인까지의 시간. Orca(`src/main/updater-events.ts`)와 같다: 확인에 성공하면 하루, 실패하면 한 시간 뒤다. */
const CHECK_EVERY_MS = 24 * 60 * 60 * 1000;
const RETRY_AFTER_MS = 60 * 60 * 1000;

/**
 * Windows 앱의 업데이트 카드(docs/mockups/update.html, Orca의 업데이트 카드를 따른다).
 * 화면을 열 때와 그 뒤 하루마다 확인한다. 받기를 시작하면 더 확인하지 않는다. 브라우저에서 열면 아무것도 하지 않는다.
 * Windows: 설치는 설치 파일을 띄우고 앱을 끝낸다. 설치 파일이 새 판을 다시 띄운다.
 */
export function UpdateCard() {
  const { t } = useTranslation();
  const [release, setRelease] = useState<Release | null>(null);
  const [step, setStep] = useState<Step | null>(null);
  const next = useRef<ReturnType<typeof setTimeout>>(undefined);

  function fail(retry: () => void) {
    return (err: unknown) => setStep({ kind: 'error', message: String(err), retry });
  }

  function find() {
    clearTimeout(next.current);
    check().then(
      (found) => {
        setRelease(found);
        setStep(found && { kind: 'available' });
        next.current = setTimeout(find, CHECK_EVERY_MS);
      },
      (err: unknown) => {
        fail(find)(err);
        next.current = setTimeout(find, RETRY_AFTER_MS);
      },
    );
  }

  function download(found: Release) {
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
      .then(() => setStep({ kind: 'ready' }), fail(() => download(found)));
  }

  function install(found: Release) {
    found.install().catch(fail(() => install(found)));
  }

  useEffect(() => {
    if (isTauri()) find();
    return () => clearTimeout(next.current);
  }, []);

  if (!step) return null;
  const version = release && t('update.version', { version: release.version });
  return (
    <div className="pop update-pop" role="complementary" aria-label={t(`update.${step.kind}`)} aria-live="polite">
      <h3>
        {t(`update.${step.kind}`)}
        <span className="spacer" />
        <button type="button" className="btn ghost sm icon" aria-label={t('common.close')} onClick={() => setStep(null)}>
          ×
        </button>
      </h3>
      {step.kind === 'available' && release && (
        <>
          <p className="hint">{version}</p>
          <button type="button" className="btn primary" onClick={() => download(release)}>
            {t('update.update')}
          </button>
        </>
      )}
      {step.kind === 'downloading' && (
        <>
          <p className="hint">{version}</p>
          <span className="meter">
            <i style={{ width: `${step.percent}%` }} />
          </span>
          <p className="hint">{t('update.percent', { percent: step.percent })}</p>
        </>
      )}
      {step.kind === 'ready' && release && (
        <>
          <p className="hint">{version}</p>
          <button type="button" className="btn primary" onClick={() => install(release)}>
            {t('update.restart')}
          </button>
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
