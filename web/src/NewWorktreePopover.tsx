import { useState, type FormEvent } from 'react';
import { useTranslation } from 'react-i18next';
import { createWorktree } from './api';
import type { RepoView } from './gen/RepoView';
import { useOutsideClose } from './popover';
import { folderName } from './repos';

/** 새 워크트리(목업 workspace.html 2번). 폴더는 `<워크스페이스 폴더>/<저장소 이름>/<브랜치 마지막 마디>`이고, 데몬이 같은 규칙으로 만든다. */
export function NewWorktreePopover(props: { repo: RepoView; workspaceDir: string; onCreated: (repo: RepoView) => void; onClose: () => void }) {
  const { t } = useTranslation();
  const box = useOutsideClose<HTMLFormElement>(props.onClose);
  const { repo } = props;
  const [branch, setBranch] = useState('');
  const [base, setBase] = useState(repo.worktrees.find((w) => w.original)?.branch ?? repo.branches[0] ?? 'HEAD');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const leaf = branch.trim().split('/').pop() ?? '';
  const sep = props.workspaceDir.includes('\\') ? '\\' : '/';
  const folder = [props.workspaceDir.replace(/[\\/]+$/, ''), folderName(repo.path), leaf].join(sep);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    try {
      props.onCreated(await createWorktree(repo.id, { branch, base }));
    } catch (err) {
      setError((err as Error).message);
      setBusy(false);
    }
  }

  return (
    <form ref={box} className="pop worktree-pop" aria-label={t('repo.newWorktree')} onSubmit={submit} onKeyDown={(e) => e.key === 'Escape' && props.onClose()}>
      <h2>{t('repo.newWorktree')}</h2>
      <div className="field">
        <label htmlFor="wt-branch">{t('repo.branch')}</label>
        <input id="wt-branch" className="mono" value={branch} onChange={(e) => setBranch(e.target.value)} spellCheck={false} autoComplete="off" autoFocus />
      </div>
      <div className="field">
        <label htmlFor="wt-base">{t('repo.base')}</label>
        <select id="wt-base" value={base} onChange={(e) => setBase(e.target.value)}>
          {(repo.branches.length > 0 ? repo.branches : [base]).map((b) => (
            <option key={b}>{b}</option>
          ))}
        </select>
      </div>
      {leaf && (
        <p className="hint mono" translate="no">
          {t('repo.folder', { path: folder })}
        </p>
      )}
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      <div className="pop-actions">
        <button type="button" className="btn ghost" onClick={props.onClose}>
          {t('common.cancel')}
        </button>
        <button type="submit" className="btn primary" disabled={busy || leaf === ''}>
          {t('repo.create')}
        </button>
      </div>
    </form>
  );
}
