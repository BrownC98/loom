import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { createInvite, getPhone, removeDevice, startServe } from './api';
import type { InviteView } from './gen/InviteView';
import type { PhoneView } from './gen/PhoneView';
import { ago } from './i18n';

/** 기기 줄의 연결 시각: 「10. 22. 오후 5:39」. */
const SHORT_TIME: Intl.DateTimeFormatOptions = { month: 'numeric', day: 'numeric', hour: 'numeric', minute: '2-digit' };
/** 코드 상자가 열린 동안 폰이 짝지었는지 묻는 간격. */
const INVITE_POLL_MS = 2000;

/**
 * 설정 「휴대폰」 탭(목업 docs/mockups/phone-pairing.html ②③, 2026-10-22 사람 결정 Q364 A, 사람 지시 "핸드폰 연결은 설정에 별도 탭을 만들것").
 * QR·코드로 폰을 짝짓고, 짝지은 기기를 끊는다. Tailscale이 준비되지 않았으면 할 일을 보인다.
 */
export function PhoneTab() {
  const { t } = useTranslation();
  const [view, setView] = useState<PhoneView | null>(null);
  const [invite, setInvite] = useState<{ view: InviteView; until: number } | null>(null);
  const [now, setNow] = useState(Date.now());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  const refresh = useCallback(() => getPhone().then(setView, (e: Error) => setError(e.message)), []);
  useEffect(() => void refresh(), [refresh]);

  // 코드 상자가 열린 동안: 남은 시간을 1초마다 다시 그리고, 폰이 썼거나 시간이 지나면 상자를 닫는다.
  useEffect(() => {
    if (!invite) return;
    const tick = setInterval(() => setNow(Date.now()), 1000);
    const poll = setInterval(
      () =>
        getPhone().then((next) => {
          setView(next);
          if (!next.inviteOpen) setInvite(null);
        }, () => {}),
      INVITE_POLL_MS,
    );
    return () => {
      clearInterval(tick);
      clearInterval(poll);
    };
  }, [invite]);

  const act = (work: () => Promise<unknown>) => {
    setBusy(true);
    setError('');
    work()
      .catch((e: Error) => setError(e.message))
      .finally(() => setBusy(false));
  };
  const open = () =>
    act(() =>
      createInvite().then((view) => {
        setNow(Date.now());
        setInvite({ view, until: Date.now() + view.ttlSecs * 1000 });
      }),
    );

  if (!view) return error ? <p className="hint">{error}</p> : null;
  const tailnet = view.tailnet;
  const left = invite ? Math.max(0, Math.ceil((invite.until - now) / 1000)) : 0;

  return (
    <>
      <section>
        <h4>{t('pairing.connect')}</h4>
        {tailnet.kind === 'ready' ? (
          <div className="phone-ready">
            <span className={`dot ${tailnet.serving ? 'done' : 'waiting'}`} />
            <span>
              {t('pairing.ready')} · <code>{tailnet.host}</code> · {tailnet.serving ? t('pairing.serveOn') : t('pairing.serveOff')}
            </span>
          </div>
        ) : (
          <div className="phone-ready">
            <span className="dot" />
            <span>{tailnet.kind === 'missing' ? t('pairing.missing') : t('pairing.stopped')}</span>
          </div>
        )}
        {tailnet.kind !== 'ready' && (
          <>
            <ol className="hint phone-steps">
              <li>{t('pairing.step1')}</li>
              <li>{t('pairing.step2')}</li>
            </ol>
            <span>
              <button type="button" className="btn sm" disabled={busy} onClick={() => act(refresh)}>
                {t('pairing.recheck')}
              </button>
            </span>
          </>
        )}
        {tailnet.kind === 'ready' && !tailnet.serving && (
          <>
            <span className="hint">{t('pairing.serveHint', { port: location.port })}</span>
            <span>
              <button type="button" className="btn sm primary" disabled={busy} onClick={() => act(() => startServe().then(setView))}>
                {t('pairing.serve')}
              </button>
            </span>
          </>
        )}
        {tailnet.kind === 'ready' && tailnet.serving && !invite && (
          <span>
            <button type="button" className="btn sm primary" disabled={busy} onClick={open}>
              {t('pairing.connect')}
            </button>
          </span>
        )}
        {invite && (
          <div className="pair-box">
            <img className="qr" alt={t('pairing.qrAlt')} src={`data:image/svg+xml;utf8,${encodeURIComponent(invite.view.qrSvg)}`} />
            <div className="pair-text">
              <b>{t('pairing.scan')}</b>
              <span className="hint">{t('pairing.typeCode')}</span>
              <span className="pair-code" translate="no">
                {invite.view.code}
              </span>
              <span className="hint">{t('pairing.left', { min: Math.floor(left / 60), sec: left % 60 })}</span>
              <span>
                <button type="button" className="btn sm" onClick={() => setInvite(null)}>
                  {t('common.close')}
                </button>
              </span>
            </div>
          </div>
        )}
        {error && <span className="error">{error}</span>}
      </section>
      <section>
        <h4>{t('pairing.devices')}</h4>
        {view.devices.length === 0 ? (
          <p className="hint">{t('pairing.noDevices')}</p>
        ) : (
          <div className="rows">
            {view.devices.map((d) => (
              <div className="row" key={d.id}>
                <span className="row-main">
                  <span className="name">{d.label}</span>
                  <span className="meta">
                    {t('pairing.deviceMeta', { paired: new Date(d.pairedAt).toLocaleString(undefined, SHORT_TIME), seen: ago(d.lastSeen, now) })}
                  </span>
                </span>
                <span />
                <span className="row-actions">
                  <button type="button" className="btn ghost sm" disabled={busy} onClick={() => act(() => removeDevice(d.id).then(refresh))}>
                    {t('pairing.disconnect')}
                  </button>
                </span>
              </div>
            ))}
          </div>
        )}
      </section>
    </>
  );
}
