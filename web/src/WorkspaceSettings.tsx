import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { FieldReset } from './FieldReset';
import type { SettingsView } from './gen/SettingsView';

/** 절대 경로: `/…`, `C:\…`, `\\서버\…`. 데몬도 같은 규칙으로 거절한다. */
const ABSOLUTE = /^(\/|[A-Za-z]:[\\/]|\\\\)/;

/** 「워크스페이스」 탭(목업 workspace.html 4번). 글 입력 하나다. 입력에서 나가거나 Enter를 누르면 저장한다. */
export function WorkspaceTab({ settings, onSettings }: { settings: SettingsView; onSettings: (next: SettingsView) => void }) {
  const { t } = useTranslation();
  const saved = settings.workspaceDir ?? '';
  const [draft, setDraft] = useState(saved);
  const [error, setError] = useState('');
  useEffect(() => setDraft(saved), [saved]);

  const commit = () => {
    const value = draft.trim();
    if (value === saved) return setError('');
    if (!ABSOLUTE.test(value)) return setError(t('problem.repo.notAbsolute', { value }));
    setError('');
    onSettings({ ...settings, workspaceDir: value });
  };

  return (
    <>
      <p className="hint">{t('workspace.desc')}</p>
      <section>
        <h4>
          <label htmlFor="ws-dir">{t('workspace.folder')}</label>
          <FieldReset path={['workspaceDir']} />
        </h4>
        <div className="field">
          <input
            id="ws-dir"
            className="mono"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onBlur={commit}
            onKeyDown={(e) => e.key === 'Enter' && commit()}
            spellCheck={false}
            autoComplete="off"
            aria-invalid={error !== ''}
            aria-describedby="ws-dir-note"
          />
        </div>
        <span id="ws-dir-note" className={error ? 'error' : 'hint'} role={error ? 'alert' : undefined}>
          {error || t('workspace.hint')}
        </span>
      </section>
    </>
  );
}
