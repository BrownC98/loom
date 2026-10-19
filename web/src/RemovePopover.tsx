import { useEffect, useRef, useState } from 'react';
import { removeNode } from './api';
import type { NodeView } from './gen/NodeView';

/** 노드 제거 확인(목업 「노드 제거」). 되돌릴 수 없으므로 끝난 노드도 묻는다. 첫 포커스는 [취소]이고, Esc와 팝오버 밖 누르기가 닫는다. */
export function RemovePopover(props: { view: NodeView; childNames: string[]; onClose: () => void }) {
  const box = useRef<HTMLDivElement>(null);
  const onClose = useRef(props.onClose);
  onClose.current = props.onClose;
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const outside = (e: PointerEvent) => {
      if (!box.current?.contains(e.target as Node)) onClose.current();
    };
    document.addEventListener('pointerdown', outside, true);
    return () => document.removeEventListener('pointerdown', outside, true);
  }, []);

  async function remove() {
    setBusy(true);
    try {
      await removeNode(props.view.id);
      props.onClose();
    } catch (err) {
      setError((err as Error).message);
      setBusy(false);
    }
  }

  const { childNames } = props;
  return (
    <div
      ref={box}
      className="pop confirm nodrag nowheel"
      role="dialog"
      aria-labelledby="rm-title"
      onKeyDown={(e) => e.key === 'Escape' && props.onClose()}
    >
      <h2 id="rm-title">
        노드 제거<span>{props.view.name}</span>
      </h2>
      <ul>
        <li>터미널 기록과 받은 메시지를 지운다.</li>
        {props.view.state.kind === 'running' && <li>실행 중인 프로세스를 끝낸다.</li>}
        {childNames.length > 0 && (
          <li>
            자식 노드 {childNames.length}개({childNames.join(', ')})는 부모 없이 계속 돈다.
          </li>
        )}
      </ul>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      <div className="pop-actions">
        <button type="button" className="btn ghost" onClick={props.onClose} autoFocus>
          취소
        </button>
        <button type="button" className="btn danger" onClick={remove} disabled={busy}>
          노드 제거
        </button>
      </div>
    </div>
  );
}
