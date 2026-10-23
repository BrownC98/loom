import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { deleteWorktree, getWorktreeChanges } from './api';
import type { NodeView } from './gen/NodeView';
import type { RepoId } from './gen/RepoId';
import type { RepoView } from './gen/RepoView';
import { useDismiss } from './popover';
import type { Group } from './repos';

/** 워크트리 삭제 경고(목업 workspace.html 3번, Orca 「Delete Workspace」). 잃는 것을 모두 보인다. 첫 포커스는 [취소]다. */
export function DeleteWorktreePopover(props: { repo: RepoId; group: Group; views: NodeView[]; onDeleted: (repo: RepoView) => void; onClose: () => void }) {
  const { t } = useTranslation();
  const box = useDismiss<HTMLDivElement>(props.onClose);
  const { group } = props;
  const [changes, setChanges] = useState<string[] | null>(null);
  const [deleteBranch, setDeleteBranch] = useState(false);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const running = props.views.filter((v) => v.state.kind === 'running');

  useEffect(() => {
    getWorktreeChanges(props.repo, group.path).then(setChanges, (err: Error) => setError(err.message));
  }, [props.repo, group.path]);

  async function remove() {
    setBusy(true);
    try {
      props.onDeleted(await deleteWorktree(props.repo, group.path, deleteBranch));
    } catch (err) {
      setError((err as Error).message);
      setBusy(false);
    }
  }

  return (
    <div ref={box} className="pop confirm wide nodrag nowheel" role="alertdialog" aria-labelledby="wt-del-title">
      <h2 id="wt-del-title">
        {t('deleteWorktree.action')}
        <span translate="no">{group.label}</span>
      </h2>
      <p className="hint mono" translate="no">
        {t('deleteWorktree.folder', { path: group.path })}
      </p>
      {changes === null ? (
        // 변경을 읽지 못하면 아래 오류 글이 대신 보인다.
        !error && <p className="hint">{t('deleteWorktree.checking')}</p>
      ) : changes.length === 0 ? (
        <p className="hint">{t('deleteWorktree.clean')}</p>
      ) : (
        <>
          <p className="error">{t('deleteWorktree.lost')}</p>
          <pre className="changes" translate="no">
            {changes.join('\n')}
          </pre>
        </>
      )}
      {(running.length > 0 || props.views.length > 0) && (
        <ul>
          {running.length > 0 && <li>{t('deleteWorktree.stop', { n: running.length, names: running.map((v) => v.name).join(', ') })}</li>}
          {props.views.length > 0 && <li>{t('deleteWorktree.remove', { n: props.views.length })}</li>}
        </ul>
      )}
      {group.branch && (
        <label className="check">
          <input type="checkbox" checked={deleteBranch} onChange={(e) => setDeleteBranch(e.target.checked)} />
          {t('deleteWorktree.branch', { branch: group.branch })}
        </label>
      )}
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      <div className="pop-actions">
        <button type="button" className="btn ghost" onClick={props.onClose} autoFocus>
          {t('common.cancel')}
        </button>
        <button type="button" className="btn danger" onClick={remove} disabled={busy}>
          {t('deleteWorktree.delete')}
        </button>
      </div>
    </div>
  );
}
