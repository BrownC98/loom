import { useState, type FormEvent } from 'react';
import { useTranslation } from 'react-i18next';
import { addRepo } from './api';
import type { RepoView } from './gen/RepoView';
import { useOutsideClose } from './popover';

/** 저장소 추가(목업 workspace.html 1번). 경로를 글로 적는다. 같은 저장소를 다시 더하면 그 저장소를 돌려받는다. */
export function AddRepoPopover(props: { onAdded: (repo: RepoView) => void; onClose: () => void }) {
  const { t } = useTranslation();
  const box = useOutsideClose<HTMLFormElement>(props.onClose);
  const [path, setPath] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    try {
      props.onAdded(await addRepo({ path }));
    } catch (err) {
      setError((err as Error).message);
      setBusy(false);
    }
  }

  return (
    <form ref={box} className="pop side-pop" aria-label={t('tree.addRepo')} onSubmit={submit} onKeyDown={(e) => e.key === 'Escape' && props.onClose()}>
      <h2>{t('tree.addRepo')}</h2>
      <div className="field">
        <label htmlFor="repo-path">{t('repo.path')}</label>
        <input id="repo-path" className="mono" value={path} onChange={(e) => setPath(e.target.value)} placeholder="/home/me/projects/app" spellCheck={false} autoComplete="off" autoFocus />
      </div>
      {error ? (
        <p className="error" role="alert">
          {error}
        </p>
      ) : (
        <p className="hint">{t('repo.pathHint')}</p>
      )}
      <div className="pop-actions">
        <button type="button" className="btn ghost" onClick={props.onClose}>
          {t('common.cancel')}
        </button>
        <button type="submit" className="btn primary" disabled={busy || path.trim() === ''}>
          {t('repo.add')}
        </button>
      </div>
    </form>
  );
}
