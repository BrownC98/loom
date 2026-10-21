import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { moveNodes } from './api';
import { rowStateKey } from './accountSwitch';
import type { MovableNode } from './gen/MovableNode';
import type { Provider } from './gen/Provider';
import { Modal } from './Modal';
import { PROVIDER_LABEL } from './usage';

/** 계정을 바꿀 때 뜨는 확인 창(목업 account-switch.html ①). 새 계정은 이미 저장했다.
 *  [새 노드만]은 그대로 두고, [모두 바꾸기]는 노드를 옮긴다. Esc와 바깥 누르기는 `onCancel`이다. */
export function SwitchDialog(props: { provider: Provider; to: string; nodes: MovableNode[]; onKeep: () => void; onCancel: () => void }) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const provider = PROVIDER_LABEL[props.provider];
  const moveAll = async () => {
    setBusy(true);
    setError('');
    try {
      await moveNodes(props.provider);
      props.onKeep();
    } catch (err) {
      setError((err as Error).message);
      setBusy(false);
    }
  };
  return (
    <Modal className="pop switch-dialog" label={t('switch.dialog.title', { provider, to: props.to })} onClose={props.onCancel} outsideClose>
      <div className="switch-body">
        <h3>{t('switch.dialog.title', { provider, to: props.to })}</h3>
        <span>{t('switch.dialog.question', { provider, count: props.nodes.length })}</span>
        <ul className="switch-list">
          {props.nodes.map((n) => (
            <li key={n.id}>
              <b>{n.name}</b>
              <span className="st">{t(rowStateKey(n.busy))}</span>
            </li>
          ))}
        </ul>
        {error && (
          <p className="error" role="alert">
            {error}
          </p>
        )}
        <div className="pop-actions">
          <button className="btn sm" disabled={busy} onClick={props.onKeep}>
            {t('switch.dialog.newOnly')}
          </button>
          <button className="btn sm primary" disabled={busy} onClick={moveAll} autoFocus>
            {t('switch.dialog.moveAll')}
          </button>
        </div>
      </div>
    </Modal>
  );
}
