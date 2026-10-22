import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { allowFirewall, createInvite, getPhone, removeDevice, startServe } from './api';
import type { InviteView } from './gen/InviteView';
import type { PhoneView } from './gen/PhoneView';
import { ago } from './i18n';

/** 기기 줄의 연결 시각: 「10. 22. 오후 5:39」. */
const SHORT_TIME: Intl.DateTimeFormatOptions = { month: 'numeric', day: 'numeric', hour: 'numeric', minute: '2-digit' };
/** 코드 상자가 열린 동안 폰이 짝지었는지 묻는 간격. */
const INVITE_POLL_MS = 2000;

/**
 * 설정 「휴대폰」 탭(목업 docs/mockups/phone-pairing.html ②③, phone-direct.html ②③, 사람 결정 Q364 A·Q414·Q418 C,
 * 사람 지시 "핸드폰 연결은 설정에 별도 탭을 만들것", "앱 연동은 orca처럼 tailscail 독립적으로 구현하라").
 * 폰 문 주소와 QR로 Android 앱을 짝짓고, 짝지은 기기를 끊는다. 브라우저(iPhone)는 선택인 Tailscale Serve 길을 쓴다.
 */
export function PhoneTab() {
  const { t } = useTranslation();
  const [view, setView] = useState<PhoneView | null>(null);
  const [invite, setInvite] = useState<{ view: InviteView; until: number } | null>(null);
  const [now, setNow] = useState(Date.now());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  /** tailnet이 Serve를 허용하지 않아 CLI가 찍은 허용 페이지. */
  const [consent, setConsent] = useState('');

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

  const turnOnServe = () =>
    act(() =>
      startServe().then((served) => {
        setConsent(served.kind === 'needsConsent' ? served.url : '');
        return refresh();
      }),
    );

  if (!view) return error ? <p className="hint">{error}</p> : null;
  const tailnet = view.tailnet;
  const serving = tailnet.kind === 'ready' && tailnet.serving;
  const left = invite ? Math.max(0, Math.ceil((invite.until - now) / 1000)) : 0;
  const door = (ip: string) => (view.port == null ? ip : `${ip}:${view.port}`);
  const lan = view.addresses.filter((a) => !a.tailnet).map((a) => door(a.ip));
  const remote = view.addresses.filter((a) => a.tailnet).map((a) => door(a.ip));

  return (
    <>
      <section>
        <h4>{t('pairing.connect')}</h4>
        <div className="phone-addr">
          <div>
            <span className="key">{t('pairing.lan')}</span>
            <span className={`dot ${lan.length > 0 ? 'done' : 'waiting'}`} />
            {lan.length > 0 ? <code>{lan.join(' · ')}</code> : <span>{t('pairing.noAddress')}</span>}
          </div>
          <div>
            <span className="key">Tailscale</span>
            <span className={`dot ${remote.length > 0 ? 'done' : ''}`} />
            {remote.length > 0 ? (
              <>
                <code>{remote.join(' · ')}</code>
                <span className="hint">{t('pairing.outside')}</span>
              </>
            ) : (
              <span className="hint">{t('pairing.noTailnet')}</span>
            )}
          </div>
        </div>
        <span className="hint">{view.addresses.length > 0 ? t('pairing.where') : t('pairing.noAddressHint')}</span>
        {!invite && (
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
        {view.firewall && (
          <span className="hint">
            {t('pairing.firewallHint')}{' '}
            <button type="button" className="btn sm" disabled={busy} onClick={() => act(allowFirewall)}>
              {t('pairing.firewall')}
            </button>
          </span>
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
      <section>
        <h4>{t('pairing.browser')}</h4>
        <div className="phone-ready">
          <span className={`dot ${serving ? 'done' : ''}`} />
          <span>
            {tailnet.kind === 'ready' ? (
              <>
                <code>{tailnet.host}</code> · {tailnet.serving ? t('pairing.serveOn') : t('pairing.serveOff')}
              </>
            ) : tailnet.kind === 'missing' ? (
              t('pairing.missing')
            ) : (
              t('pairing.stopped')
            )}
          </span>
        </div>
        <span className="hint">{t('pairing.browserHint')}</span>
        {tailnet.kind === 'ready' && !tailnet.serving && (
          <>
            {consent && (
              <span className="hint">
                {t('pairing.consent')}{' '}
                <a href={consent} target="_blank" rel="noopener">
                  {t('pairing.consentLink')}
                </a>
              </span>
            )}
            <span>
              <button type="button" className="btn sm" disabled={busy} onClick={turnOnServe}>
                {t('pairing.serve')}
              </button>
            </span>
          </>
        )}
        {tailnet.kind !== 'ready' && (
          <span>
            <button type="button" className="btn sm" disabled={busy} onClick={() => act(refresh)}>
              {t('pairing.recheck')}
            </button>
          </span>
        )}
      </section>
    </>
  );
}
