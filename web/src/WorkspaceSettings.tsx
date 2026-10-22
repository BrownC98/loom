import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { getDistro, getLocations } from './api';
import { FieldReset } from './FieldReset';
import type { SettingsView } from './gen/SettingsView';

/** 절대 경로: `/…`, `C:\…`, `\\서버\…`. 데몬도 같은 규칙으로 거절한다. */
const ABSOLUTE = /^(\/|[A-Za-z]:[\\/]|\\\\)/;

/**
 * 「워크스페이스」 탭(목업 workspace.html 4번). 위치마다 글 입력 하나다. 입력에서 나가거나 Enter를 누르면 저장한다.
 * WSL 배포판이 있으면 배포판마다 칸을 더한다(목업 wsl-place.html ⑤). WSL 저장소의 워크트리는 그 배포판 안에 있어야 한다.
 */
export function WorkspaceTab({ settings, onSettings }: { settings: SettingsView; onSettings: (next: SettingsView) => void }) {
  const { t } = useTranslation();
  const [distros, setDistros] = useState<string[]>([]);
  // 배포판마다 기본 폴더(`<홈>/loom/workspaces`). 데몬이 배포판을 켜서 읽는다.
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
      <p className="hint">{t(places ? 'workspace.descPlaces' : 'workspace.desc')}</p>
      <DirField
        id="ws-dir"
        label={places ? t('place.pc') : t('workspace.folder')}
        reset={['workspaceDir']}
        saved={settings.workspaceDir ?? ''}
        note={places ? null : t('workspace.hint')}
        onSave={(workspaceDir) => onSettings({ ...settings, workspaceDir })}
      />
      {distros.map((distro, i) => (
        <DirField
          key={distro}
          id={`ws-dir-${i}`}
          label={t('place.wslDistro', { distro })}
          reset={['wslWorkspaceDirs', distro]}
          saved={settings.wslWorkspaceDirs[distro] ?? defaults[distro] ?? ''}
          note={i === distros.length - 1 ? t('workspace.hint') : null}
          onSave={(dir) => onSettings({ ...settings, wslWorkspaceDirs: { ...settings.wslWorkspaceDirs, [distro]: dir } })}
        />
      ))}
    </>
  );
}

/** 폴더 칸 하나. `note`는 칸 아래의 안내다(목업은 마지막 칸 아래 한 번). 틀린 값은 안내 자리에 보인다. */
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
