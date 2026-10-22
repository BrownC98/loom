import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { FieldReset } from './FieldReset';
import type { SettingsView } from './gen/SettingsView';

/** 설정 「일반」의 원격 칸(설계 「원격」). Tailscale Serve로 온 폰을 받을 로그인 하나다. 입력에서 나가거나 Enter를 누르면 저장한다. */
export function RemoteFields({ settings, onSettings }: { settings: SettingsView; onSettings: (next: SettingsView) => void }) {
  const { t } = useTranslation();
  const saved = settings.remoteLogin ?? '';
  const [draft, setDraft] = useState(saved);
  useEffect(() => setDraft(saved), [saved]);

  const commit = () => {
    const value = draft.trim();
    if (value !== saved) onSettings({ ...settings, remoteLogin: value === '' ? null : value });
  };

  return (
    <>
      <section>
        <h4>
          <label htmlFor="remote-login">{t('remote.login')}</label>
          <FieldReset path={['remoteLogin']} />
        </h4>
        <span className="hint">{t('remote.desc')}</span>
        <div className="field">
          <input
            id="remote-login"
            className="mono"
            type="email"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onBlur={commit}
            onKeyDown={(e) => e.key === 'Enter' && commit()}
            spellCheck={false}
            autoComplete="off"
            aria-describedby="remote-login-note"
          />
        </div>
        <span id="remote-login-note" className="hint">
          {t('remote.hint')}
        </span>
      </section>
      <section>
        <h4>{t('remote.steps')}</h4>
        <ol className="hint">
          {(['step1', 'step2', 'step3', 'step4'] as const).map((step) => (
            <li key={step}>{t(`remote.${step}`)}</li>
          ))}
        </ol>
      </section>
    </>
  );
}
