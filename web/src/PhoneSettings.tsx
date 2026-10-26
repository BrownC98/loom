import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { allowFirewall, createInvite, getPhone, removeDevice, startServe } from './api';
import type { InviteView } from './gen/InviteView';
import type { PhoneView } from './gen/PhoneView';
import { ago, dayAndTime } from './i18n';

export function PhoneTab(props: { viewVersion: number }) {
  const { t } = useTranslation();
  const [view, setView] = useState<PhoneView | null>(null);
  const [invite, setInvite] = useState<InviteView | null>(null);
  const [now, setNow] = useState(Date.now());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [consent, setConsent] = useState('');

  const refresh = useCallback(
    () =>
      getPhone().then(
        (next) => {
          setView(next);
          if (!next.inviteOpen) setInvite(null);
        },
        (e: Error) => setError(e.message),
      ),
    [],
  );
  useEffect(() => void refresh(), [refresh, props.viewVersion]);

  useEffect(() => {
    if (!invite) return;
    const tick = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(tick);
  }, [invite]);

  const left = invite ? Math.max(0, Math.ceil((invite.until - now) / 1000)) : 0;
  useEffect(() => {
    if (!invite || left > 0) return;
    setInvite(null);
    void refresh();
  }, [invite, left, refresh]);

  const act = (work: () => Promise<unknown>) => {
    setBusy(true);
    setError('');
    work()
      .catch((e: Error) => setError(e.message))
      .finally(() => setBusy(false));
  };
  const open = () =>
    act(() =>
      createInvite().then((next) => {
        setNow(Date.now());
        setInvite(next);
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
            <img className="qr" alt={t('pairing.qrAlt')} src={`data:image/svg+xml;utf8,${encodeURIComponent(invite.qrSvg)}`} />
            <div className="pair-text">
              <b>{t('pairing.scan')}</b>
              <span className="hint">{t('pairing.typeCode')}</span>
              <span className="pair-code" translate="no">
                {invite.code}
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
                    {t('pairing.deviceMeta', { paired: dayAndTime(d.pairedAt), seen: ago(d.lastSeen, now) })}
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
