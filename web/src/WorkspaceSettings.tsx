import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { getDistro, getLocations } from './api';
import { FieldReset } from './FieldReset';
import type { SettingsView } from './gen/SettingsView';

const ABSOLUTE = /^(\/|[A-Za-z]:[\\/]|\\\\)/;

export function WorkspaceFields({ settings, onSettings }: { settings: SettingsView; onSettings: (next: SettingsView) => void }) {
  const { t } = useTranslation();
  const [distros, setDistros] = useState<string[]>([]);
  const [defaults, setDefaults] = useState<Record<string, string>>({});
  useEffect(() => {
    getLocations().then(({ distros }) => {
      setDistros(distros);
      for (const distro of distros) getDistro(distro).then(({ workspace }) => setDefaults((d) => ({ ...d, [distro]: workspace })), () => {});
    }, () => {});
  }, []);
  const places = distros.length > 0;

  return (
    <>
      <DirField
        id="ws-dir"
        label={places ? t('workspace.folderAt', { place: t('place.pc') }) : t('workspace.folder')}
        reset={['workspaceDir']}
        saved={settings.workspaceDir ?? ''}
        note={places ? null : t('workspace.hint')}
        onSave={(workspaceDir) => onSettings({ ...settings, workspaceDir })}
      />
      {distros.map((distro, i) => (
        <DirField
          key={distro}
          id={`ws-dir-${i}`}
          label={t('workspace.folderAt', { place: t('place.wslDistro', { distro }) })}
          reset={['wslWorkspaceDirs', distro]}
          saved={settings.wslWorkspaceDirs[distro] ?? defaults[distro] ?? ''}
          note={i === distros.length - 1 ? t('workspace.hintPlaces') : null}
          onSave={(dir) => onSettings({ ...settings, wslWorkspaceDirs: { ...settings.wslWorkspaceDirs, [distro]: dir } })}
        />
      ))}
    </>
  );
}

function DirField(props: { id: string; label: string; reset: Parameters<typeof FieldReset>[0]['path']; saved: string; note: string | null; onSave: (dir: string) => void }) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState(props.saved);
  const [error, setError] = useState('');
  useEffect(() => setDraft(props.saved), [props.saved]);

  const commit = () => {
    const value = draft.trim();
    if (value === props.saved) return setError('');
    if (!ABSOLUTE.test(value)) return setError(t('problem.repo.notAbsolute', { value }));
    setError('');
    props.onSave(value);
  };
  const note = `${props.id}-note`;

  return (
    <section>
      <h4>
        <label htmlFor={props.id}>{props.label}</label>
        <FieldReset path={props.reset} />
      </h4>
      <div className="field">
        <input
          id={props.id}
          className="mono"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => e.key === 'Enter' && commit()}
          spellCheck={false}
          autoComplete="off"
          aria-invalid={error !== ''}
          aria-describedby={error || props.note ? note : undefined}
        />
      </div>
      {(error || props.note) && (
        <span id={note} className={error ? 'error' : 'hint'} role={error ? 'alert' : undefined}>
          {error || props.note}
        </span>
      )}
    </section>
  );
}
