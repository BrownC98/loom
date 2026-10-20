import { useState, type FormEvent, type KeyboardEvent } from 'react';
import { useTranslation } from 'react-i18next';
import { addRepo, cloneRepo, createRepo } from './api';
import { childPath, FolderPicker } from './FolderPicker';
import type { RepoView } from './gen/RepoView';
import { useOutsideClose } from './popover';

const TABS = ['folder', 'clone', 'create'] as const;
type Tab = (typeof TABS)[number];
const SUBMIT = { folder: 'repo.add', clone: 'repo.clone', create: 'repo.create' } as const;

/** git이 `git clone <url>`에서 고르는 폴더 이름. 데몬의 `worktree::repo_name`과 같은 규칙이다. */
function repoName(url: string): string {
  const leaf = url.trim().replace(/\/+$/, '').split(/[/:]/).pop() ?? '';
  return leaf.replace(/\.git$/, '');
}

/** 저장소 추가(목업 repository.html). 기존 폴더, URL 복제, 새 프로젝트. 같은 저장소를 다시 더하면 그 저장소를 돌려받는다. */
export function AddRepoPopover(props: { onAdded: (repo: RepoView) => void; onClose: () => void }) {
  const { t } = useTranslation();
  const box = useOutsideClose<HTMLFormElement>(props.onClose);
  const [tab, setTab] = useState<Tab>('folder');
  const [path, setPath] = useState('');
  const [url, setUrl] = useState('');
  const [name, setName] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);

  function choose(next: Tab) {
    setTab(next);
    setError('');
  }

  function onTabKey(e: KeyboardEvent) {
    const step = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0;
    if (step === 0 || busy) return;
    e.preventDefault();
    const next = TABS[(TABS.indexOf(tab) + step + TABS.length) % TABS.length];
    choose(next);
    document.getElementById(`repo-tab-${next}`)?.focus();
  }

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError('');
    try {
      const parent = path.trim();
      const added = tab === 'folder' ? await addRepo({ path: parent }) : tab === 'clone' ? await cloneRepo({ url, parent }) : await createRepo({ parent, name });
      props.onAdded(added);
    } catch (err) {
      setError((err as Error).message);
      setBusy(false);
    }
  }

  const target = tab === 'folder' ? path.trim() : tab === 'clone' ? (repoName(url) && childPath(path.trim(), repoName(url))) : name.trim() && childPath(path.trim(), name.trim());
  const ready = path.trim() !== '' && (tab === 'folder' || (tab === 'clone' ? url.trim() !== '' : name.trim() !== ''));

  return (
    <form ref={box} className="pop side-pop add-repo" aria-label={t('tree.addRepo')} aria-busy={busy} onSubmit={submit} onKeyDown={(e) => e.key === 'Escape' && props.onClose()}>
      <h2>{t('tree.addRepo')}</h2>
      <div className="tabs" role="tablist" aria-label={t('tree.addRepo')} onKeyDown={onTabKey}>
        {TABS.map((each) => (
          <button key={each} id={`repo-tab-${each}`} type="button" role="tab" aria-selected={tab === each} aria-controls="repo-panel" tabIndex={tab === each ? 0 : -1} disabled={busy && tab !== each} onClick={() => choose(each)}>
            {t(`repo.tab.${each}`)}
          </button>
        ))}
      </div>
      <div id="repo-panel" role="tabpanel" aria-labelledby={`repo-tab-${tab}`} className="tab-panel">
        {tab === 'clone' && (
          <div className="field">
            <label htmlFor="repo-url">{t('repo.url')}</label>
            <input id="repo-url" name="url" className="mono" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://github.com/team/app.git" disabled={busy} spellCheck={false} autoComplete="off" autoFocus translate="no" />
          </div>
        )}
        {tab === 'create' && (
          <div className="field">
            <label htmlFor="repo-name">{t('repo.name')}</label>
            <input id="repo-name" name="name" value={name} onChange={(e) => setName(e.target.value)} placeholder="my-app" disabled={busy} spellCheck={false} autoComplete="off" autoFocus />
          </div>
        )}
        <FolderPicker key={tab} id="repo-path" label={t(tab === 'folder' ? 'repo.path' : 'repo.parent')} disabled={busy} autoFocus={tab === 'folder'} onPick={setPath} />
      </div>
      {error ? (
        <p className="error" role="alert">
          {error}
        </p>
      ) : (
        target && (
          <p className="hint">
            {t(tab === 'folder' ? 'repo.target' : 'repo.folder', { path: target })}
          </p>
        )
      )}
      <div className="pop-actions">
        <button type="button" className="btn ghost" onClick={props.onClose}>
          {t('common.cancel')}
        </button>
        <button type="submit" className="btn primary" disabled={busy || !ready}>
          {t(busy ? `repo.busy.${tab}` : SUBMIT[tab])}
        </button>
      </div>
    </form>
  );
}
